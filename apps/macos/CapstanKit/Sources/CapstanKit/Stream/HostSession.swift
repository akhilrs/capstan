import Foundation

public enum SessionError: Sendable, Equatable {
    /// ssh itself failed (exit 255).
    case ssh(tail: UntrustedText)
    /// The host wrote an error line and ended the stream.
    case stream(code: String, message: UntrustedText)
    /// The command ended with another exit code.
    case exit(Int32, tail: UntrustedText)
    /// The stream ended, or a line could not be read, without a reason from the host.
    case interrupted
}

public enum StopReason: Sendable, Equatable {
    case user
    case unsupportedVersion(Int)
    case hostRejected(code: String?, tail: UntrustedText)
    case cstanMissing(tail: UntrustedText)
    case repeatedFailure
}

public enum ConnectionState: Sendable, Equatable {
    case connecting
    case live(since: Date, lastLineAt: Date)
    case noHeartbeat(since: Date)
    case reconnecting(attempt: Int, retryAt: Date, lastError: SessionError?)
    case stopped(StopReason)
}

/// Receives what a session reads. `apply` is awaited before the session reads on.
public protocol StreamConsumer: Sendable {
    /// A batch of lines that ends at a cursor line, or the partial batch before a reconnect.
    func apply(host: UUID, lines: [StreamLine]) async
    func connection(host: UUID, _ state: ConnectionState) async
    /// The cursor to resume from; nil to start with a full snapshot.
    func resumeCursor(host: UUID) async -> Cursor?
    /// Forget the stored cursor: the host does not accept it.
    func dropCursor(host: UUID) async
}

/// One `cstan events` stream over ssh, kept alive with backoff.
public actor HostSession {
    static let backoffSeconds: [TimeInterval] = [1, 2, 4, 8, 16, 32, 60]
    /// The server's heartbeat interval; an assumption taken from the events engine, not stated in the protocol.
    static let serverHeartbeatMs = 15_000
    static let liveResetSeconds: TimeInterval = 60
    static let maxFailuresInRow = 5
    static let refreshCoalesceSeconds: TimeInterval = 1
    /// Interim (plan-73 moves the re-read into the stream): a refresh restarts the stream at most this often, with
    /// every project invalidated meanwhile coalesced into the one restart.
    static let refreshGapSeconds: TimeInterval = 60
    static let liveRepublishSeconds: TimeInterval = 5
    /// Error codes that mean this client cannot get a stream from the host by trying again.
    static let rejectingCodes: Set<String> = ["unauthorized", "forbidden", "unknown_command", "invalid_request"]

    private enum Outcome {
        case superseded
        case stop(StopReason)
        case restartNow
        case retry(SessionError, countsAsFailure: Bool)
    }

    private let config: HostConfig
    private let runner: any ProcessRunner
    private let consumer: any StreamConsumer
    private let clock: any SessionClock
    private let jitter: @Sendable () -> Double
    private let decoder = LineDecoder()

    public private(set) var state: ConnectionState = .connecting

    private var generation = 0
    private var runTask: Task<Void, Never>?
    private var connectionId = 0
    private var currentProcess: (any RunningProcess)?
    private var watchdogTask: Task<Void, Never>?
    private var watchdogFired = false
    private var terminatedConnections: Set<Int> = []
    private var refreshRestartRequested = false
    /// The next connect follows a refresh restart: the host stays live, as nothing failed.
    private var refreshRestartPending = false
    /// The last child started; the next one waits for it to be gone, so a host never has two ssh sessions.
    private var previousProcess: (any RunningProcess, Int)?
    private var lastLineAt: Date
    private var liveSince: Date?
    private var lastLivePublish: Date?

    private var cursor: Cursor?
    private var cursorLoaded = false
    private var attempt = 0
    private var failures = 0

    private var pendingRefresh: Set<String> = []
    private var refreshTask: Task<Void, Never>?
    private var lastRefreshRestart: Date?

    /// `jitter` returns the factor applied to a backoff delay (0.8 to 1.2 in use).
    public init(
        config: HostConfig, runner: any ProcessRunner, consumer: any StreamConsumer, clock: any SessionClock,
        jitter: @escaping @Sendable () -> Double = { Double.random(in: 0.8...1.2) }
    ) {
        self.config = config
        self.runner = runner
        self.consumer = consumer
        self.clock = clock
        self.jitter = jitter
        self.lastLineAt = clock.now()
    }

    // MARK: Control

    public func start() {
        guard runTask == nil else { return }
        launch()
    }

    /// Ends the stream and does not reconnect.
    public func stop() async {
        generation += 1
        cancelWork()
        refreshRestartPending = false
        runTask?.cancel()
        runTask = nil
        refreshTask = nil
        pendingRefresh = []
        await publish(.stopped(.user))
    }

    /// The manual retry: drops any wait, resets the backoff and connects now.
    public func restart() {
        cancelWork()
        refreshRestartPending = false
        runTask?.cancel()
        attempt = 0
        failures = 0
        launch()
    }

    /// Asks for the given projects to be read again. v1 has no remote item read, so the stream restarts without
    /// those projects in its cursor and the host sends their snapshots again.
    public func refresh(projectIds: Set<String>) {
        guard runTask != nil, !projectIds.isEmpty else { return }
        pendingRefresh.formUnion(projectIds)
        guard refreshTask == nil else { return }
        let gen = generation
        refreshTask = Task { await self.runRefresh(generation: gen) }
    }

    private func launch() {
        generation += 1
        let gen = generation
        runTask = Task { await self.run(generation: gen) }
    }

    private func cancelWork() {
        watchdogTask?.cancel()
        watchdogTask = nil
        refreshTask?.cancel()
        refreshTask = nil
        if let process = currentProcess { terminate(process, connection: connectionId) }
        currentProcess = nil
    }

    /// Ends a child once: the stop, refresh and watchdog paths and the read loop's own cleanup may all reach for it.
    private func terminate(_ process: any RunningProcess, connection id: Int) {
        guard terminatedConnections.insert(id).inserted else { return }
        process.terminate()
    }

    private func publish(_ new: ConnectionState) async {
        state = new
        await consumer.connection(host: config.id, new)
    }

    // MARK: Run loop

    private func run(generation gen: Int) async {
        while gen == generation {
            let outcome = await connectOnce(generation: gen)
            guard gen == generation else { return }
            switch outcome {
            case .superseded:
                return
            case .stop(let reason):
                runTask = nil
                await publish(.stopped(reason))
                return
            case .restartNow:
                continue
            case .retry(let error, let countsAsFailure):
                if countsAsFailure {
                    failures += 1
                    if failures >= HostSession.maxFailuresInRow {
                        runTask = nil
                        await publish(.stopped(.repeatedFailure))
                        return
                    }
                }
                attempt += 1
                let base = HostSession.backoffSeconds[min(attempt, HostSession.backoffSeconds.count) - 1]
                let delay = base * jitter()
                await publish(.reconnecting(
                    attempt: attempt, retryAt: clock.now().addingTimeInterval(delay), lastError: error))
                guard gen == generation else { return }
                do {
                    try await clock.sleep(seconds: delay)
                } catch {
                    return
                }
            }
        }
    }

    private func connectOnce(generation gen: Int) async -> Outcome {
        connectionId += 1
        let id = connectionId
        watchdogFired = false
        refreshRestartRequested = false
        let quiet = refreshRestartPending
        refreshRestartPending = false
        if let (old, oldId) = previousProcess {
            terminate(old, connection: oldId)
            _ = await old.exitCode()
            previousProcess = nil
            guard gen == generation else { return .superseded }
        }
        if !quiet {
            liveSince = nil
            lastLivePublish = nil
            await publish(.connecting)
            guard gen == generation else { return .superseded }
        }
        let resume = await currentCursor()
        guard gen == generation else { return .superseded }

        let process: any RunningProcess
        do {
            let arguments = RemoteCommand.ssh(config, RemoteCommand.eventsArguments(config, cursor: resume))
            process = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
        } catch {
            return .retry(.ssh(tail: hostText("could not start ssh")), countsAsFailure: true)
        }
        currentProcess = process
        previousProcess = (process, id)
        lastLineAt = clock.now()
        let watchdog = Task { await self.watch(connection: id) }
        watchdogTask = watchdog

        var batch: [StreamLine] = []
        var errorLine: (code: String, message: UntrustedText)?
        var decision: Outcome?
        var dropStoredCursor = false

        do {
            readLoop: for try await data in process.lines {
                guard gen == generation, !refreshRestartRequested else { break readLoop }
                lastLineAt = clock.now()
                let line = try decoder.decode(data)
                switch line {
                case .heartbeat, .unknownKind:
                    await noteLive()
                case .error(let code, let message):
                    if code == "bad_cursor", resume != nil {
                        dropStoredCursor = true
                        decision = .restartNow
                    } else if code == "bad_cursor" {
                        // No cursor was sent, so dropping it changes nothing: back off instead of looping.
                        decision = .retry(.stream(code: code, message: message), countsAsFailure: true)
                    } else if HostSession.rejectingCodes.contains(code) {
                        decision = .stop(.hostRejected(code: code, tail: hostText(process.stderrTail())))
                    } else {
                        errorLine = (code, message)
                    }
                case .cursor(let next):
                    batch.append(line)
                    await noteLive()
                    await consumer.apply(host: config.id, lines: batch)
                    batch = []
                    if !refreshRestartRequested {
                        cursor = next
                        cursorLoaded = true
                    }
                default:
                    batch.append(line)
                    await noteLive()
                }
                if decision != nil { break readLoop }
            }
        } catch let error as LineDecodeError {
            if case .unsupportedVersion(let version) = error {
                decision = .stop(.unsupportedVersion(version))
            } else {
                decision = .retry(.interrupted, countsAsFailure: true)
            }
        } catch {
            decision = .retry(.interrupted, countsAsFailure: true)
        }

        // This run's own watchdog; after a restart the field may already hold the new connection's.
        watchdog.cancel()
        if connectionId == id { watchdogTask = nil }
        guard gen == generation else {
            terminate(process, connection: id)
            return .superseded
        }
        if connectionId == id { currentProcess = nil }
        if refreshRestartRequested {
            // The partial batch is read again with the snapshots after the restart.
            terminate(process, connection: id)
            return .restartNow
        }
        if decision != nil || watchdogFired { terminate(process, connection: id) }
        if !batch.isEmpty {
            await consumer.apply(host: config.id, lines: batch)
            batch = []
            guard gen == generation else { return .superseded }
        }
        if let since = liveSince, clock.now().timeIntervalSince(since) >= HostSession.liveResetSeconds {
            attempt = 0
            failures = 0
        }
        if dropStoredCursor {
            cursor = nil
            cursorLoaded = true
            await consumer.dropCursor(host: config.id)
        }
        if watchdogFired { return .restartNow }
        if let decision { return decision }

        let code = await process.exitCode()
        guard gen == generation else { return .superseded }
        let tail = hostText(process.stderrTail())
        switch code {
        case 255:
            return .retry(.ssh(tail: tail), countsAsFailure: false)
        case 2:
            return .stop(.hostRejected(code: nil, tail: tail))
        case 127:
            return .stop(.cstanMissing(tail: tail))
        default:
            if let errorLine {
                return .retry(.stream(code: errorLine.code, message: errorLine.message), countsAsFailure: true)
            }
            if code == 0 { return .retry(.interrupted, countsAsFailure: false) }
            return .retry(.exit(code, tail: tail), countsAsFailure: true)
        }
    }

    private func currentCursor() async -> Cursor? {
        if !cursorLoaded {
            cursor = await consumer.resumeCursor(host: config.id)
            cursorLoaded = true
        }
        return cursor
    }

    private func hostText(_ raw: String) -> UntrustedText {
        UntrustedText(raw: raw, verified: true, truncated: false)
    }

    /// Records that a line came in: the connection is live, and the state is refreshed now and then.
    private func noteLive() async {
        let now = clock.now()
        if let since = liveSince {
            if let last = lastLivePublish, now.timeIntervalSince(last) < HostSession.liveRepublishSeconds { return }
            lastLivePublish = now
            await publish(.live(since: since, lastLineAt: now))
        } else {
            liveSince = now
            lastLivePublish = now
            await publish(.live(since: now, lastLineAt: now))
        }
    }

    // MARK: Heartbeat

    private var heartbeatTimeout: TimeInterval {
        3 * max(Double(config.pollMs) / 1000, Double(HostSession.serverHeartbeatMs) / 1000)
    }

    private func watch(connection id: Int) async {
        while id == connectionId, !Task.isCancelled {
            let remaining = lastLineAt.addingTimeInterval(heartbeatTimeout).timeIntervalSince(clock.now())
            if remaining <= 0 {
                await publish(.noHeartbeat(since: clock.now()))
                guard id == connectionId else { return }
                watchdogFired = true
                if let process = currentProcess { terminate(process, connection: id) }
                return
            }
            do {
                try await clock.sleep(seconds: remaining)
            } catch {
                return
            }
        }
    }

    // MARK: Refresh

    private func runRefresh(generation gen: Int) async {
        do {
            try await clock.sleep(seconds: HostSession.refreshCoalesceSeconds)
            if let last = lastRefreshRestart {
                let wait = HostSession.refreshGapSeconds - clock.now().timeIntervalSince(last)
                if wait > 0 { try await clock.sleep(seconds: wait) }
            }
        } catch {
            return
        }
        guard gen == generation else { return }
        let ids = pendingRefresh
        pendingRefresh = []
        refreshTask = nil
        let base: Cursor?
        if cursorLoaded {
            base = cursor
        } else {
            base = await consumer.resumeCursor(host: config.id)
        }
        guard gen == generation else { return }
        do {
            cursor = try base?.removing(ids)
        } catch {
            cursor = nil
        }
        cursorLoaded = true
        lastRefreshRestart = clock.now()
        if let process = currentProcess {
            refreshRestartRequested = true
            refreshRestartPending = true
            terminate(process, connection: connectionId)
        }
    }
}
