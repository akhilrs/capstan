import Foundation

/// Why nothing was sent, or why the answer is unknown. The decision may still have reached the host.
public enum NotSentReason: Sendable, Equatable {
    case couldNotStart
    case timeout
    /// ssh itself failed (exit 255); the text is its standard error.
    case ssh(tail: UntrustedText)
}

/// What the person is told to do about a refusal.
public enum RefusalHint: Sendable, Equatable {
    /// The item changed: load it again.
    case refresh
    /// The item is no longer waiting.
    case gone
    /// Answer in a terminal.
    case terminal
    /// There is no active project manager to forward to.
    case noPm
    /// Too many forwards for this item.
    case limit
    /// The client's own mistake (a reused nonce): the next request gets a new nonce.
    case `internal`
    /// The request was not valid.
    case invalid
    /// The host or the key does not allow it.
    case host
    case generic

    public init(code: DecideErrorCode) {
        switch code {
        case .hashMismatch: self = .refresh
        case .notWaiting: self = .gone
        case .needsTerminal: self = .terminal
        case .noActivePm: self = .noPm
        case .limitReached: self = .limit
        case .nonceReused: self = .internal
        case .invalidRequest: self = .invalid
        case .forbidden, .unauthorized, .projectNotFound, .noOperatorKey, .notAnswering: self = .host
        case .other: self = .generic
        }
    }
}

public enum DecideOutcome: Sendable, Equatable {
    case applied(DecideResult)
    case forwarded(DecideResult)
    case refused(DecideRefusal, hint: RefusalHint)
    /// No answer came. The request is kept; `retry` sends it again with the same nonce.
    case notSent(reason: NotSentReason, pendingId: UUID)
    case malformed(String, pendingId: UUID?)
}

/// Sends decisions through the user's ssh and keeps the ones that got no answer.
public actor Decider {
    /// The child is stopped after this many seconds.
    public static let timeoutSeconds: TimeInterval = 20

    private enum Finish {
        case done(stdout: Data, code: Int32)
        case timeout
    }

    private let runner: any ProcessRunner
    private let pending: PendingStore
    private let clock: any SessionClock

    public init(runner: any ProcessRunner, pending: PendingStore, clock: any SessionClock) {
        self.runner = runner
        self.pending = pending
        self.clock = clock
    }

    /// Sends a new request. A value that would not be accepted is refused here and nothing is kept.
    public func send(_ request: DecideRequest, config: HostConfig) async -> DecideOutcome {
        let arguments: [String]
        do {
            arguments = try request.sshArguments(config)
        } catch let error as DecideRequestError {
            return .refused(
                DecideRefusal(
                    code: .invalidRequest, message: UntrustedText(raw: error.message, verified: true, truncated: false)),
                hint: .invalid)
        } catch {
            return .malformed("The request could not be built", pendingId: nil)
        }
        let record = PendingDecision(request: request, sentAt: clock.now())
        await pending.add(record)
        return await run(record, arguments: arguments)
    }

    /// Sends a kept request again, nonce included: if the host applied it, the answer is a repeat. The recorded
    /// host settings are not kept, so the caller passes the current ones.
    public func retry(_ id: UUID, config: HostConfig) async -> DecideOutcome {
        guard let record = await pending.record(id) else { return .malformed("There is no such pending request", pendingId: nil) }
        guard let arguments = try? record.request.sshArguments(config) else {
            await pending.remove(id)
            return .malformed("The pending request is not valid", pendingId: nil)
        }
        return await run(record, arguments: arguments)
    }

    private func run(_ record: PendingDecision, arguments: [String]) async -> DecideOutcome {
        let process: any RunningProcess
        do {
            process = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
        } catch {
            return .notSent(reason: .couldNotStart, pendingId: record.id)
        }
        let clock = self.clock
        let finish: Finish = await withTaskGroup(of: Finish?.self) { group in
            group.addTask {
                let stdout = await process.stdoutAll()
                return .done(stdout: stdout, code: await process.exitCode())
            }
            group.addTask {
                do { try await clock.sleep(seconds: Decider.timeoutSeconds) } catch { return nil }
                // Stopping the child also ends the task that waits for its output.
                process.terminate()
                return .timeout
            }
            var result: Finish = .timeout
            for await item in group {
                if let item {
                    result = item
                    group.cancelAll()
                    break
                }
            }
            return result
        }
        switch finish {
        case .timeout:
            return .notSent(reason: .timeout, pendingId: record.id)
        case .done(let stdout, let code):
            return await outcome(record, stdout: stdout, code: code, stderr: process.stderrTail())
        }
    }

    private func outcome(_ record: PendingDecision, stdout: Data, code: Int32, stderr: String) async -> DecideOutcome {
        switch code {
        case 255:
            let tail = UntrustedText(raw: stderr, verified: true, truncated: false)
            return .notSent(reason: .ssh(tail: tail), pendingId: record.id)
        case 2:
            await pending.remove(record.id)
            return .malformed("The host's cstan rejected the request as bad usage; nothing was applied", pendingId: nil)
        case 0, 1:
            guard let answer = Decider.answer(stdout) else {
                return .malformed("The host answered with something unexpected; the decision may have been applied", pendingId: record.id)
            }
            await pending.remove(record.id)
            switch answer {
            case .result(let result):
                return result.outcome == .applied ? .applied(result) : .forwarded(result)
            case .refusal(let refusal):
                return .refused(refusal, hint: RefusalHint(code: refusal.code))
            }
        default:
            return .malformed("The host ended with exit code \(code); the decision may have been applied", pendingId: record.id)
        }
    }

    /// The answer when standard output is exactly one JSON line.
    private static func answer(_ stdout: Data) -> DecideAnswer? {
        let lines = stdout.split(separator: 0x0A, omittingEmptySubsequences: true)
            .filter { line in !line.allSatisfy { byte in byte == 0x20 || byte == 0x0D || byte == 0x09 } }
        guard lines.count == 1, let line = lines.first else { return nil }
        return try? DecideAnswer(data: Data(line))
    }
}
