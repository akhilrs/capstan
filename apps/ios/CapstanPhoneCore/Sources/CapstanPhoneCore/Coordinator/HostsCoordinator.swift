import CapstanKit
import Foundation

/// A host's running session, as the coordinator sees it. `HostSession` is one; tests use a fake.
public protocol HostSessionControl: Sendable {
    func start() async
    func stop() async
    func restart() async
}

extension HostSession: HostSessionControl {}

/// What keeps a host from connecting until the person acts. Host text is never carried here.
public enum HostProblem: Sendable, Equatable {
    /// The host key is not trusted yet: the trust sheet shows its fingerprint.
    case hostKeyUnknown(HostKeyPin)
    /// Another key than the pinned one: blocked; Remove host and add it again after comparing fingerprints.
    case hostKeyChanged
    /// The host refused this iPhone's key: add the public line to `authorized_keys`, then Try again.
    case authRefused
    case unsupported(UnsupportedReason)
    /// The saved host keys cannot be read: every host waits for Reset all host keys.
    case pinsUnreadable
}

/// What the add-host form gets back for a new host.
public struct HostCandidate: Sendable, Equatable {
    public let config: HostConfig
    public let host: PhoneHost

    public init(config: HostConfig, host: PhoneHost) {
        self.config = config
        self.host = host
    }
}

/// The iPhone's host lifecycle, as the Mac's `AppModel` does it: the launch sequence, a session per enabled host with
/// generations, add (candidate, check, commit or discard), update, remove, restart, and the transport's events.
///
/// Every start and stop of a session runs inside `enqueue`, one after the other, and reads `hosts` when it runs. The
/// coordinator is the only consumer of `TransportControl.events` and subscribes before any session starts.
@MainActor
public final class HostsCoordinator: SessionControl {
    public typealias SessionMaker = @MainActor (HostConfig) -> any HostSessionControl
    public typealias Checker = @Sendable (HostConfig) -> AsyncStream<HostCheck>

    public private(set) var hosts: [HostConfig] = [] {
        didSet { onChange?() }
    }
    /// Per host: what keeps it from connecting.
    public private(set) var problems: [UUID: HostProblem] = [:] {
        didSet { onChange?() }
    }
    /// Per host (a candidate included): the key waiting for Trust or Cancel.
    public private(set) var pendingTrust: [UUID: PendingTrust] = [:] {
        didSet { onChange?() }
    }
    public private(set) var pinsUnreadable = false {
        didSet { onChange?() }
    }
    /// Called after `hosts`, `problems`, `pendingTrust` or `pinsUnreadable` changed, so the app's observable model
    /// can copy them (this package does not import Observation).
    public var onChange: (@MainActor () -> Void)?
    public private(set) var started = false

    private let transport: any TransportControl
    private let registry: PhoneHostRegistry
    private let pins: HostKeyStore
    private let phoneStore: PhoneHostStore
    private let hostsFile: HostsFile
    private let makeSession: SessionMaker
    private let checker: Checker

    private var sessions: [UUID: any HostSessionControl] = [:]
    private var generations: [UUID: Int] = [:]
    private var lifecycle: Task<Void, Never>?
    private var listener: Task<Void, Never>?

    /// `checker` runs a host's connection check; the app passes `checkHost` on the shared runner.
    public init(
        transport: any TransportControl, registry: PhoneHostRegistry, pins: HostKeyStore, phoneStore: PhoneHostStore,
        hostsFile: HostsFile, makeSession: @escaping SessionMaker, checker: @escaping Checker
    ) {
        self.transport = transport
        self.registry = registry
        self.pins = pins
        self.phoneStore = phoneStore
        self.hostsFile = hostsFile
        self.makeSession = makeSession
        self.checker = checker
    }

    /// The checker the app uses: CapstanKit's host check on the shared runner.
    public nonisolated static func checker(runner: any ProcessRunner, clock: any SessionClock) -> Checker {
        { config in checkHost(config, runner, clock: clock) }
    }

    // MARK: Running

    /// Subscribes to the transport's events, then runs the launch sequence: load both host lists, drop what is
    /// half saved, register the hosts, sweep the pins, and start a session per enabled host that nothing blocks.
    public func start() {
        guard !started else { return }
        started = true
        subscribe()
        hosts = HostStartup.run(hostsFile: hostsFile, phoneStore: phoneStore, registry: registry, pins: pins)
        // Events sent before the subscription are in the stream too; reading the latest ones here makes the blocks
        // known before any session starts.
        for host in hosts {
            if let event = transport.latestEvent(hostId: host.id) { handle(event) }
        }
        if pins.isUnreadable { handle(.pinsUnreadable) }
        startSessions()
    }

    /// Starts every enabled host that has no session and nothing blocking it.
    public func startSessions() {
        enqueue { [self] in
            guard started else { return }
            for config in hosts where config.enabled && sessions[config.id] == nil && !isBlocked(config.id) {
                await startHost(config)
            }
        }
    }

    /// Stops every session, then suspends the transport: the order the app needs before it is suspended.
    public func stopSessionsThenSuspend() {
        enqueue { [self] in
            for id in Array(sessions.keys) { await teardownHost(id) }
            transport.suspend()
        }
    }

    /// Stops everything and the event listener.
    public func stop() async {
        started = false
        listener?.cancel()
        listener = nil
        enqueue { [self] in
            for id in Array(sessions.keys) { await teardownHost(id) }
        }
        await lifecycle?.value
    }

    /// Returns once every queued start and stop has run. For tests and for an orderly exit.
    public func waitForIdle() async {
        while let current = lifecycle {
            await current.value
            if lifecycle == current { break }
        }
    }

    public var runningHosts: Set<UUID> {
        Set(sessions.keys)
    }

    // MARK: SessionControl

    /// Restarts a host's session. A changed key or unreadable pins keep it blocked; other problems clear, as the
    /// person asked to try again.
    public func restart(hostId: UUID) {
        guard !isHardBlocked(hostId) else { return }
        if case .hostKeyUnknown? = problems[hostId] { return }
        problems[hostId] = nil
        enqueue { [self] in
            guard started, let config = hosts.first(where: { $0.id == hostId }), config.enabled else { return }
            if let session = sessions[hostId] {
                await session.restart()
            } else {
                await startHost(config)
            }
        }
    }

    // MARK: Trust

    /// Pins the key the host offered and starts the host. For a candidate, the add-host form checks again.
    public func trust(hostId: UUID) {
        guard let pending = pendingTrust.removeValue(forKey: hostId) else { return }
        pins.trust(hostId: hostId, pin: pending.pin)
        if case .hostKeyUnknown? = problems[hostId] { problems[hostId] = nil }
        restart(hostId: hostId)
    }

    /// The person cancelled the trust sheet: the host stays blocked until they try again.
    public func cancelTrust(hostId: UUID) {
        pendingTrust[hostId] = nil
    }

    /// Forgets every pin after the pin file could not be read; every host then asks for trust again.
    public func resetAllPins() {
        pins.resetAll()
        pinsUnreadable = false
        problems = [:]
        pendingTrust = [:]
        startSessions()
    }

    // MARK: Hosts

    /// Registers a new host as a candidate for the add-host form. Throws `PhoneHostError` (duplicate, invalid) or
    /// the first `HostConfigError`.
    public func prepareCandidate(
        displayName: String, hostname: String, port: Int, user: String, cstanPath: String = "cstan",
        extraRoots: [String] = [], pollMs: Int
    ) throws -> HostCandidate {
        let id = registry.newHostId()
        let host = PhoneHost(id: id, hostname: hostname, port: port, user: user)
        let config = HostConfig(
            id: id, displayName: displayName, destination: host.alias, cstanPath: cstanPath, extraRoots: extraRoots,
            pollMs: pollMs)
        if let error = config.validate().first { throw error }
        try registry.registerCandidate(host)
        return HostCandidate(config: config, host: host)
    }

    /// The connection check of a host or a candidate.
    public func check(_ config: HostConfig) -> AsyncStream<HostCheck> {
        checker(config)
    }

    /// Adds a checked candidate: phone hosts are saved first, then the host list, then its session starts.
    public func commit(_ candidate: HostCandidate) {
        registry.commit(candidate.host.id)
        hosts.append(candidate.config)
        save()
        let id = candidate.config.id
        enqueue { [self] in
            guard started, sessions[id] == nil, !isBlocked(id),
                  let current = hosts.first(where: { $0.id == id }), current.enabled else { return }
            await startHost(current)
        }
    }

    /// Drops a candidate that was not added, and forgets its pins.
    public func discard(_ candidate: HostCandidate) {
        let id = candidate.host.id
        registry.discard(id)
        pendingTrust[id] = nil
        problems[id] = nil
    }

    /// Saves new settings for a host and restarts its session. A new hostname, port or user is a new place: its pin
    /// is forgotten, so the key is trusted again.
    public func updateHost(_ config: HostConfig, host: PhoneHost) throws {
        guard let index = hosts.firstIndex(where: { $0.id == config.id }), host.id == config.id else { return }
        let moved = registry.host(host.id).map { $0.identity != host.identity } ?? false
        try registry.register(host)
        if moved {
            pins.forget(hostId: host.id)
            problems[host.id] = nil
            pendingTrust[host.id] = nil
        }
        var updated = config
        updated.destination = host.alias
        hosts[index] = updated
        save()
        let id = config.id
        enqueue { [self] in
            await teardownHost(id)
            guard started, let current = hosts.first(where: { $0.id == id }), current.enabled, !isBlocked(id) else { return }
            await startHost(current)
        }
    }

    /// Removes a host: its session stops, its pins are forgotten and both lists are saved.
    public func removeHost(_ id: UUID) {
        hosts.removeAll { $0.id == id }
        registry.remove(id)
        problems[id] = nil
        pendingTrust[id] = nil
        save()
        enqueue { [self] in await teardownHost(id) }
    }

    private func save() {
        let ids = hosts.map(\.id)
        let saved = ids.compactMap { registry.host($0) }
        try? phoneStore.save(saved)
        try? hostsFile.save(hosts)
    }

    // MARK: Events

    private func subscribe() {
        let events = transport.events
        listener = Task { [weak self] in
            for await event in events {
                guard let self else { return }
                self.handle(event)
            }
        }
    }

    func handle(_ event: TransportEvent) {
        switch event {
        case .hostKeyUnknown(let id, let pin):
            pendingTrust[id] = PendingTrust(hostId: id, pin: pin)
            problems[id] = .hostKeyUnknown(pin)
            block(id)
        case .hostKeyChanged(let id):
            pendingTrust[id] = nil
            problems[id] = .hostKeyChanged
            block(id)
        case .authRefused(let id):
            problems[id] = .authRefused
            block(id)
        case .unsupported(let id, let reason):
            problems[id] = .unsupported(reason)
            block(id)
        case .pinsUnreadable:
            pinsUnreadable = true
            for host in hosts {
                problems[host.id] = .pinsUnreadable
                block(host.id)
            }
        }
    }

    /// Stops a host's session so it does not retry into the same wall; the person's action starts it again.
    private func block(_ id: UUID) {
        guard sessions[id] != nil else { return }
        enqueue { [self] in await teardownHost(id) }
    }

    private func isBlocked(_ id: UUID) -> Bool {
        pinsUnreadable || problems[id] != nil
    }

    private func isHardBlocked(_ id: UUID) -> Bool {
        if pinsUnreadable { return true }
        if case .hostKeyChanged? = problems[id] { return true }
        return false
    }

    // MARK: Sessions

    private func enqueue(_ op: @escaping @MainActor () async -> Void) {
        let previous = lifecycle
        lifecycle = Task {
            await previous?.value
            await op()
        }
    }

    private func startHost(_ config: HostConfig) async {
        let id = config.id
        generations[id] = (generations[id] ?? 0) + 1
        let session = makeSession(config)
        sessions[id] = session
        await session.start()
    }

    private func teardownHost(_ id: UUID) async {
        generations[id] = (generations[id] ?? 0) + 1
        guard let session = sessions.removeValue(forKey: id) else { return }
        await session.stop()
    }

    /// The current generation of a host's session; it grows on every start and stop.
    public func generation(of id: UUID) -> Int {
        generations[id] ?? 0
    }
}
