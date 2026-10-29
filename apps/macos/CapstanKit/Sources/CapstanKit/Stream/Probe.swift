import Foundation

public struct HostCapabilities: Sendable, Equatable {
    /// ssh got to the host and the script ran.
    public let reachable: Bool
    /// The host's cstan has the `events` verb.
    public let events: Bool
    /// The host's cstan has the `decide` verb.
    public let decide: Bool
    public let error: SessionError?

    public init(reachable: Bool, events: Bool, decide: Bool, error: SessionError? = nil) {
        self.reachable = reachable
        self.events = events
        self.decide = decide
        self.error = error
    }
}

public enum HostCheckStep: Sendable, Equatable {
    case connect, capstan, projects, firstSnapshot
}

public enum HostCheckStatus: Sendable, Equatable {
    case passed, warning, failed
}

/// One line of the connection check. `text` is written by the client, except for host text it quotes.
public struct HostCheck: Sendable, Equatable {
    public let step: HostCheckStep
    public let status: HostCheckStatus
    public let text: String

    public init(step: HostCheckStep, status: HostCheckStatus, text: String) {
        self.step = step
        self.status = status
        self.text = text
    }
}

private let capabilityVerbs = ["events", "decide"]
private let snapshotWaitSeconds: TimeInterval = 10

// `cstan help <verb>` exits 0 for a verb the host's cstan has and 2 for one it lacks, so the exit code is the probe.
/// Asks the host which verbs its cstan has, in one ssh call. Only `cap <verb>` lines of the output count.
public func probeCapabilities(_ config: HostConfig, _ runner: any ProcessRunner) async -> HostCapabilities {
    let cstan = RemoteCommand.cstanWord(config)
    let script = capabilityVerbs
        .map { "\(cstan) help \($0) >/dev/null 2>&1 && echo cap \($0)" }
        .joined(separator: "; ") + "; true"
    let arguments = RemoteCommand.sshArguments(config, remote: RemoteCommand.shScript(script))
    let process: any RunningProcess
    do {
        process = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
    } catch {
        return HostCapabilities(
            reachable: false, events: false, decide: false,
            error: .ssh(tail: UntrustedText(raw: "could not start ssh", verified: true, truncated: false)))
    }
    let output = await process.stdoutAll()
    let code = await process.exitCode()
    let tail = UntrustedText(raw: process.stderrTail(), verified: true, truncated: false)
    if code == 255 {
        return HostCapabilities(reachable: false, events: false, decide: false, error: .ssh(tail: tail))
    }
    let caps = Set(
        String(decoding: output, as: UTF8.self).split(whereSeparator: { $0 == "\n" || $0 == "\r" })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { $0.hasPrefix("cap ") }
            .map { String($0.dropFirst(4)) })
    return HostCapabilities(
        reachable: true, events: caps.contains("events"), decide: caps.contains("decide"),
        error: code == 0 ? nil : .exit(code, tail: tail))
}

/// The steps of a first connection test, each reported as it finishes. The stream ends after a failed step.
public func checkHost(
    _ config: HostConfig, _ runner: any ProcessRunner, clock: any SessionClock = SystemClock()
) -> AsyncStream<HostCheck> {
    AsyncStream { continuation in
        let task = Task {
            await runChecks(config, runner, clock) { _ = continuation.yield($0) }
            continuation.finish()
        }
        continuation.onTermination = { _ in task.cancel() }
    }
}

private func runChecks(
    _ config: HostConfig, _ runner: any ProcessRunner, _ clock: any SessionClock,
    _ report: @Sendable (HostCheck) -> Void
) async {
    // 1. ssh reaches the host.
    let reach: any RunningProcess
    do {
        reach = try runner.start(
            URL(fileURLWithPath: RemoteCommand.sshPath), RemoteCommand.sshArguments(config, remote: "true"))
    } catch {
        report(HostCheck(step: .connect, status: .failed, text: "Could not start ssh"))
        return
    }
    let reachCode = await reach.exitCode()
    if reachCode != 0 {
        let tail = reach.stderrTail().trimmingCharacters(in: .whitespacesAndNewlines)
        let hint = reachCode == 255
            ? "ssh could not connect. If this host is new, connect once in a terminal to accept its key."
            : "The command failed with exit code \(reachCode)."
        report(HostCheck(step: .connect, status: .failed, text: tail.isEmpty ? hint : hint + " " + tail))
        return
    }
    report(HostCheck(step: .connect, status: .passed, text: "Connected"))

    // 2. the host has cstan with the events verb.
    let caps = await probeCapabilities(config, runner)
    guard caps.events else {
        report(HostCheck(
            step: .capstan, status: .failed,
            text: caps.reachable ? "cstan was not found on the host, or it is too old (no events verb)"
                : "The host could not be reached"))
        return
    }
    report(HostCheck(
        step: .capstan, status: caps.decide ? .passed : .warning,
        text: caps.decide ? "cstan found" : "cstan found, but it cannot send decisions"))

    // 3 and 4. a short events run: the projects, then the first snapshot.
    let events: any RunningProcess
    do {
        let arguments = RemoteCommand.ssh(config, RemoteCommand.eventsArguments(config, cursor: nil))
        events = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
    } catch {
        report(HostCheck(step: .projects, status: .failed, text: "Could not start ssh"))
        return
    }
    let tally = Tally()
    await withTaskGroup(of: Void.self) { group in
        group.addTask { await tally.read(events) }
        group.addTask {
            try? await clock.sleep(seconds: snapshotWaitSeconds)
            tally.timeUp()
        }
        await tally.waitDone()
        events.terminate()
        group.cancelAll()
    }
    let result = tally.result()
    if let failure = result.failure {
        report(HostCheck(step: .projects, status: .failed, text: failure))
        return
    }
    if result.total == 0 {
        report(HostCheck(step: .projects, status: .warning, text: "The host listed no projects"))
    } else {
        report(HostCheck(
            step: .projects, status: result.ready == result.total ? .passed : .warning,
            text: "\(result.ready) of \(result.total) projects ready"))
    }
    if result.snapshot {
        report(HostCheck(step: .firstSnapshot, status: .passed, text: "First snapshot received"))
    } else {
        report(HostCheck(
            step: .firstSnapshot, status: .warning,
            text: "No snapshot within \(Int(snapshotWaitSeconds)) seconds"))
    }
}

/// What the short events run saw.
private final class Tally: @unchecked Sendable {
    struct Result {
        var total = 0
        var ready = 0
        var snapshot = false
        var failure: String?
    }

    private let lock = NSLock()
    private var value = Result()
    private var timedOut = false
    private let done = Latch<Void>()

    func result() -> Result {
        lock.withLock { value }
    }

    func waitDone() async {
        await done.wait()
    }

    func timeUp() {
        lock.withLock { timedOut = true }
        done.set(())
    }

    func read(_ process: any RunningProcess) async {
        let decoder = LineDecoder()
        do {
            for try await data in process.lines {
                switch try decoder.decode(data) {
                case .project(let project):
                    lock.withLock {
                        value.total += 1
                        if project.state == .ready { value.ready += 1 }
                    }
                case .snapshotEnd:
                    lock.withLock { value.snapshot = true }
                    done.set(())
                    return
                case .error(let code, _):
                    lock.withLock { value.failure = "The host refused the stream (\(code))" }
                    done.set(())
                    return
                default:
                    break
                }
            }
        } catch {
            lock.withLock { value.failure = "The stream could not be read" }
            done.set(())
            return
        }
        let code = await process.exitCode()
        lock.withLock {
            if !timedOut { value.failure = "The stream ended early (exit code \(code))" }
        }
        done.set(())
    }
}
