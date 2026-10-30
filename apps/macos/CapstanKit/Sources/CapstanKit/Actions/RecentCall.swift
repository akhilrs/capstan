import Foundation

/// The refusals of a call that would not be accepted by `cstan recent`; nothing is sent.
public enum RecentCallError: Error, Sendable, Equatable {
    case project
    case epoch
    case head
}

/// One outcome page of `cstan recent`: the newest outcome events of a project at or below a head.
public struct RecentPage: Sendable, Equatable {
    public let projectId: String
    public let epoch: String
    public let head: Int64
    /// Newest first, by `(seq, k)`.
    public let events: [FeedEvent]
    /// False only when the read reached the first row of the ledger.
    public let truncated: Bool

    public init(projectId: String, epoch: String, head: Int64, events: [FeedEvent], truncated: Bool) {
        self.projectId = projectId
        self.epoch = epoch
        self.head = head
        self.events = events
        self.truncated = truncated
    }
}

/// What a call to `cstan recent` came to.
public enum RecentResult: Sendable, Equatable {
    case page(RecentPage)
    /// The ledger is at another epoch or head than the one sent.
    case resync(epoch: String, head: Int64)
    /// The host has no usable `recent` command: the overview shows what happened since the connection.
    case sessionOnly
    /// The host limits how often history is read; what is held stays.
    case rateLimited
    /// Any other refusal, or no answer. The code and the message are host text, cleaned.
    case error(code: String, message: String)
}

/// `cstan recent --project <id> --epoch <hex> --head <n> --json` over ssh, with its answer parsed.
public struct RecentCall: Sendable, Equatable {
    /// The child is stopped after this many seconds.
    public static let timeoutSeconds: TimeInterval = 10
    /// How a rejected answer starts when the host limits the rate of the reads.
    public static let rateLimitedPrefix = "rate_limited: "
    /// The window of the host's rate limit, which a retry waits for.
    public static let rateLimitWindowSeconds: TimeInterval = 60
    /// The text a cstan without the command prints on standard error (exit 2).
    static let unknownCommandText = "cstan: unknown command: recent"

    public let projectId: String
    public let epoch: String
    public let head: Int64

    public init(projectId: String, epoch: String, head: Int64) {
        self.projectId = projectId
        self.epoch = epoch
        self.head = head
    }

    /// The arguments after `cstan`. Throws the local refusal when a value would not be accepted.
    public func remoteArguments() throws -> [String] {
        guard RecentCall.isProjectId(projectId) else { throw RecentCallError.project }
        guard RecentCall.isEpoch(epoch) else { throw RecentCallError.epoch }
        guard head >= 0 else { throw RecentCallError.head }
        return ["recent", "--project", projectId, "--epoch", epoch, "--head", String(head), "--json"]
    }

    /// The arguments of `/usr/bin/ssh` for this call.
    public func sshArguments(_ config: HostConfig) throws -> [String] {
        RemoteCommand.ssh(config, try remoteArguments())
    }

    static func isProjectId(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard !bytes.isEmpty, bytes.count <= 64, bytes[0] != 0x2D else { return false }
        return bytes.allSatisfy { byte in
            (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || byte == 0x5F || byte == 0x2D
        }
    }

    static func isEpoch(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard bytes.count == 16 else { return false }
        return bytes.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
    }

    // MARK: Running

    private enum Finish {
        case done(stdout: Data, code: Int32)
        case timeout
    }

    /// Starts the call and waits at most `timeoutSeconds`. A call that cannot start or does not answer is an error
    /// result; it never throws.
    public func run(runner: any ProcessRunner, config: HostConfig, clock: any SessionClock) async -> RecentResult {
        let arguments: [String]
        do {
            arguments = try sshArguments(config)
        } catch {
            return .error(code: "invalid_call", message: "The history request is not valid")
        }
        let process: any RunningProcess
        do {
            process = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
        } catch {
            return .error(code: "not_started", message: "The history request could not start")
        }
        let timedOut = Latch<Bool>()
        let finish: Finish = await withTaskGroup(of: Finish?.self) { group in
            group.addTask {
                let stdout = await process.stdoutAll()
                return .done(stdout: stdout, code: await process.exitCode())
            }
            group.addTask {
                do { try await clock.sleep(seconds: RecentCall.timeoutSeconds) } catch { return nil }
                // Stopping the child also ends the task that waits for its output; the flag says which came first.
                timedOut.set(true)
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
        if timedOut.peek() != nil { return .error(code: "timeout", message: "The host did not answer in time") }
        switch finish {
        case .timeout:
            return .error(code: "timeout", message: "The host did not answer in time")
        case .done(let stdout, let code):
            return RecentCall.result(stdout: stdout, stderr: process.stderrTail(), exitCode: code)
        }
    }

    // MARK: Answer

    /// Reads what the child printed. Exit 0 is a page; exit 1 is a refusal or a resync answer, as one JSON line; 255 is
    /// ssh itself. Only the usage text of a cstan without the command, or the `unknown_command` and `invalid_request`
    /// codes, mean session only; any other output is an error that carries its text.
    public static func result(stdout: Data, stderr: String, exitCode: Int32) -> RecentResult {
        if exitCode == 255 { return .error(code: "ssh", message: plain(stderr)) }
        let printed = String(decoding: stdout, as: UTF8.self)
        if printed.contains(unknownCommandText) || stderr.contains(unknownCommandText) { return .sessionOnly }
        guard let value = lastObject(stdout) else {
            let shown = stderr.isEmpty ? printed : stderr
            return .error(code: "unexpected_output", message: plain(shown))
        }
        if case .object? = value["error"] {
            return refusal(value)
        }
        if case .array(let rows)? = value["events"] {
            return page(value, rows: rows)
        }
        if let epoch = text(value["epoch"]), case .int(let head)? = value["head"] {
            return .resync(epoch: plain(epoch), head: head)
        }
        return .error(code: "unexpected_output", message: "The history answer is not valid")
    }

    private static func refusal(_ value: JSONValue) -> RecentResult {
        let code = text(value["error"]?["code"]) ?? ""
        let message = text(value["error"]?["message"]) ?? ""
        switch code {
        case "unknown_command", "invalid_request":
            return .sessionOnly
        case "rejected" where message.hasPrefix(rateLimitedPrefix):
            return .rateLimited
        default:
            return .error(code: plain(code), message: plain(message))
        }
    }

    private static func page(_ value: JSONValue, rows: [JSONValue]) -> RecentResult {
        guard let projectId = text(value["projectId"]), let epoch = text(value["epoch"]),
              case .int(let head)? = value["head"] else {
            return .error(code: "malformed", message: "The history answer is not valid")
        }
        var truncated = false
        if case .bool(let flag)? = value["truncated"] { truncated = flag }
        // A row this app cannot read is left out; the others are shown.
        let events: [FeedEvent] = rows.compactMap { (row: JSONValue) -> FeedEvent? in try? FeedEvent(json: row) }
        let newestFirst = events.sorted { (left: FeedEvent, right: FeedEvent) -> Bool in
            left.seq == right.seq ? left.k > right.k : left.seq > right.seq
        }
        return .page(
            RecentPage(projectId: projectId, epoch: epoch, head: head, events: newestFirst, truncated: truncated))
    }

    private static func text(_ value: JSONValue?) -> String? {
        if case .string(let text)? = value { return text }
        return nil
    }

    /// The last non-blank line when it is a JSON object.
    private static func lastObject(_ stdout: Data) -> JSONValue? {
        let lines = stdout.split(separator: 0x0A, omittingEmptySubsequences: true)
            .filter { line in !line.allSatisfy { byte in byte == 0x20 || byte == 0x0D || byte == 0x09 } }
        guard let line = lines.last,
              let value = try? JSONDecoder().decode(JSONValue.self, from: Data(line)),
              case .object = value else { return nil }
        return value
    }

    /// Host text on one line, cleaned and cut.
    private static func plain(_ text: String) -> String {
        let cleaned = UntrustedText(raw: text, verified: true, truncated: false).displayPlain
        return String(cleaned.prefix(256))
    }
}
