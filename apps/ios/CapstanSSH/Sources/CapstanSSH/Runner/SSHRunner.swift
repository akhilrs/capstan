import CapstanKit
import CapstanPhoneCore
import Foundation
import NIOCore
import NIOPosix
import NIOSSH

/// Why `start` refused before opening anything.
public enum SSHRunnerStartError: Error, Sendable, Equatable {
    /// The executable is not `RemoteCommand.sshPath`.
    case notSSH
    /// The arguments are not a list `RemoteCommand.sshArguments` writes.
    case unparsableArguments
    /// A key file was named; the phone uses its own key only.
    case identityFile
    /// The destination is no registered host's alias.
    case unknownDestination
}

/// The iPhone's `ProcessRunner`: each `start` runs the remote command on an SSH exec channel of the host's shared
/// connection, with the phone's key and pinned host keys. One per app process; the app calls `shutdown()` on exit.
public final class SSHRunner: ProcessRunner, TransportControl, @unchecked Sendable {
    public let events: AsyncStream<TransportEvent>
    private let sink: AsyncStream<TransportEvent>.Continuation

    private let registry: PhoneHostRegistry
    private let vault: KeyVault
    private let trust: HostKeyStore
    private let tuning: TransportTuning
    private let clock: any SessionClock
    private let hook: any TransportHook
    private let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)

    private let lock = NSLock()
    private var connections: [UUID: HostConnection] = [:]
    private var latest: [UUID: TransportEvent] = [:]
    private var optionsObserver: (@Sendable ([SocketOption]) -> Void)?

    public init(
        registry: PhoneHostRegistry, vault: KeyVault, trust: HostKeyStore, tuning: TransportTuning,
        clock: any SessionClock, hook: any TransportHook = DefaultHook()
    ) {
        self.registry = registry
        self.vault = vault
        self.trust = trust
        self.tuning = tuning
        self.clock = clock
        self.hook = hook
        // Unbounded, so an event sent before the coordinator subscribes is still read.
        let (stream, sink) = AsyncStream<TransportEvent>.makeStream(bufferingPolicy: .unbounded)
        events = stream
        self.sink = sink
    }

    /// Tests read the socket options applied at bootstrap.
    func observeSocketOptions(_ observer: @escaping @Sendable ([SocketOption]) -> Void) {
        lock.withLock { optionsObserver = observer }
    }

    // MARK: ProcessRunner

    public func start(_ executable: URL, _ arguments: [String]) throws -> any RunningProcess {
        guard executable.path == RemoteCommand.sshPath else { throw SSHRunnerStartError.notSSH }
        guard let invocation = RemoteCommand.invocation(from: arguments) else {
            throw SSHRunnerStartError.unparsableArguments
        }
        guard invocation.identityFile == nil else { throw SSHRunnerStartError.identityFile }
        guard let host = registry.resolve(invocation.destination) else { throw SSHRunnerStartError.unknownDestination }

        let process = SSHProcess()
        if trust.isUnreadable {
            emit(.pinsUnreadable, hostId: host.id)
            process.finish(failure: "ssh: \(host.display): the saved host keys cannot be read")
            return process
        }
        let (connection, isNew) = connectionFor(host)
        if isNew { connect(connection) }
        openExec(invocation.remote, process: process, on: connection)
        return process
    }

    // MARK: TransportControl

    /// The host's latest event. It clears on the host's next successful start, and when the person trusted the
    /// unknown key or forgot the changed one.
    public func latestEvent(hostId: UUID) -> TransportEvent? {
        if trust.isUnreadable { return .pinsUnreadable }
        guard let event = lock.withLock({ latest[hostId] }) else { return nil }
        switch event {
        case .hostKeyUnknown: return trust.pin(for: hostId) == nil ? event : nil
        case .hostKeyChanged: return trust.pin(for: hostId) != nil ? event : nil
        case .pinsUnreadable: return nil
        default: return event
        }
    }

    /// Closes every connection at once; open channels end with 255. The next `start` reconnects.
    public func suspend() {
        let all: [HostConnection] = lock.withLock {
            let open = Array(connections.values)
            connections = [:]
            return open
        }
        for connection in all { connection.close(reason: "ssh: \(connection.host.display): the app was suspended") }
    }

    /// Ends every connection and stops the event loop.
    public func shutdown() {
        suspend()
        sink.finish()
        group.shutdownGracefully { _ in }
    }

    // MARK: Connections

    private func connectionFor(_ host: PhoneHost) -> (HostConnection, Bool) {
        let (connection, isNew, replaced): (HostConnection, Bool, HostConnection?) = lock.withLock {
            var replaced: HostConnection?
            if let existing = connections[host.id] {
                if existing.host == host, existing.openChannel() { return (existing, false, nil) }
                replaced = existing
            }
            let fresh = HostConnection(host: host, loop: group.next())
            _ = fresh.openChannel()
            connections[host.id] = fresh
            return (fresh, true, replaced)
        }
        replaced?.close(reason: "ssh: \(host.display): the host settings changed")
        return (connection, isNew)
    }

    private func drop(_ connection: HostConnection, reason: String) {
        lock.withLock {
            if connections[connection.host.id] === connection { connections.removeValue(forKey: connection.host.id) }
        }
        connection.close(reason: reason)
    }

    private func connect(_ connection: HostConnection) {
        let host = connection.host
        let preferences = hook.algorithms(pinnedHostKey: trust.pin(for: host.id)?.algorithm)
        let ciphers = SSHRunner.narrow(preferences.ciphers, SupportedAlgorithms.ciphers)
        let options = hook.channelOptions()
        let observer = lock.withLock { optionsObserver }
        observer?(options)
        let tuning = self.tuning

        // The key is read off the event loop; NIO signs with it once per connection, on the loop.
        Task.detached { [self] in
            let phoneKey: PhoneKey?
            do {
                phoneKey = try vault.current()
            } catch {
                connection.fail("ssh: this iPhone's key cannot be read")
                connection.resolve(.failure(error))
                return
            }
            guard let phoneKey else {
                connection.fail("ssh: this iPhone has no key")
                connection.resolve(.failure(SSHRunnerError.noKey))
                return
            }
            let key = SSHRunner.nioKey(phoneKey)

            var bootstrap = ClientBootstrap(group: connection.loop)
                .connectTimeout(.milliseconds(Int64(tuning.connectTimeout * 1000)))
            for option in options {
                bootstrap = bootstrap.channelOption(
                    ChannelOptions.socket(SocketOptionLevel(option.level), SocketOptionName(option.name)),
                    value: SocketOptionValue(option.value))
            }
            bootstrap = bootstrap.channelInitializer { [self] channel in
                channel.eventLoop.makeCompletedFuture {
                    let configuration = SSHClientConfiguration(
                        userAuthDelegate: VaultKeyAuth(user: host.user, key: key) { [self] in authRefused(connection) },
                        serverAuthDelegate: PinnedHostKeys { [self] pin in verify(pin, connection) },
                        globalRequestDelegate: nil,
                        transportProtectionSchemes: SSHRunner.schemes(ciphers))
                    let sniffer = ServerOfferSniffer { [self] offer in
                        check(offer, preferences: preferences, ciphers: ciphers, connection)
                    }
                    let sync = channel.pipeline.syncOperations
                    try sync.addHandler(sniffer)
                    try sync.addHandler(
                        NIOSSHHandler(role: .client(configuration), allocator: channel.allocator, inboundChildChannelInitializer: nil))
                    try sync.addHandler(ConnectionErrors(connection))
                }
            }
            bootstrap.connect(host: host.hostname, port: host.port).whenComplete { [self] result in
                switch result {
                case .success(let channel):
                    channel.closeFuture.whenComplete { [self] _ in
                        drop(connection, reason: "ssh: \(host.display): the connection closed")
                    }
                case .failure(let error):
                    connection.fail("ssh: \(host.display): \(SSHRunner.describe(error))")
                    drop(connection, reason: "ssh: \(host.display): could not connect")
                }
                connection.resolve(result)
            }
        }
    }

    private func openExec(_ command: String, process: SSHProcess, on connection: HostConnection) {
        let host = connection.host
        let ended = EndOnce { [self] in channelEnded(connection) }
        let started: @Sendable () -> Void = { [self] in startSucceeded(host.id) }
        let child = connection.channel.flatMap { channel -> EventLoopFuture<Channel> in
            let promise = channel.eventLoop.makePromise(of: Channel.self)
            do {
                let ssh = try channel.pipeline.syncOperations.handler(type: NIOSSHHandler.self)
                ssh.createChannel(promise, channelType: .session) { child, _ in
                    child.eventLoop.makeCompletedFuture {
                        try child.pipeline.syncOperations.addHandler(
                            ExecHandler(
                                command: command, process: process, connection: connection, started: started,
                                ended: { ended.run() }))
                    }
                }
            } catch {
                promise.fail(error)
            }
            return promise.futureResult
        }
        let hook = self.hook
        let clock = self.clock
        child.whenComplete { result in
            switch result {
            case .success(let channel):
                process.attach { grace in _ = hook.terminate(channel, grace: grace, clock: clock) }
            case .failure:
                process.attach { _ in }
                process.finish(failure: connection.failure ?? "ssh: \(host.display): could not open a channel")
                ended.run()
            }
        }
    }

    private func channelEnded(_ connection: HostConnection) {
        guard connection.closeChannel() else { return }
        let clock = self.clock
        let idle = tuning.idleClose
        let task = Task { [self] in
            do { try await clock.sleep(seconds: idle) } catch { return }
            guard connection.isIdle else { return }
            drop(connection, reason: "ssh: \(connection.host.display): closed after being idle")
        }
        connection.setIdleTask(task)
    }

    // MARK: Events and checks

    private func emit(_ event: TransportEvent, hostId: UUID) {
        lock.withLock { latest[hostId] = event }
        sink.yield(event)
    }

    private func startSucceeded(_ hostId: UUID) {
        lock.withLock { _ = latest.removeValue(forKey: hostId) }
    }

    private func authRefused(_ connection: HostConnection) {
        connection.fail("ssh: \(connection.host.display): the host refused this iPhone's key")
        emit(.authRefused(hostId: connection.host.id), hostId: connection.host.id)
    }

    /// Runs on the event loop: the verdict comes from memory.
    private func verify(_ pin: HostKeyPin, _ connection: HostConnection) -> Bool {
        let host = connection.host
        switch trust.verdict(hostId: host.id, offered: pin) {
        case .trusted:
            return true
        case .unknown(let offered):
            connection.fail("ssh: \(host.display): the host key is not trusted yet (\(offered.fingerprint))")
            emit(.hostKeyUnknown(hostId: host.id, pin: offered), hostId: host.id)
        case .changed:
            connection.fail("ssh: \(host.display): the host key changed")
            emit(.hostKeyChanged(hostId: host.id), hostId: host.id)
        case .pinsUnreadable:
            connection.fail("ssh: \(host.display): the saved host keys cannot be read")
            emit(.pinsUnreadable, hostId: host.id)
        }
        return false
    }

    private func check(
        _ offer: ServerOffer, preferences: AlgorithmPreferences, ciphers: [String], _ connection: HostConnection
    ) -> UnsupportedReason? {
        guard let reason = offer.unsupported(preferences, ciphers: ciphers) else { return nil }
        switch reason {
        case .hostKeyType(let name): connection.fail("unsupported: host key type \(name)")
        case .noCommonAlgorithm: connection.fail("unsupported: no common algorithm")
        }
        emit(.unsupported(hostId: connection.host.id, reason), hostId: connection.host.id)
        return reason
    }

    // MARK: Helpers

    static func narrow(_ wanted: [String], _ supported: [String]) -> [String] {
        let kept = wanted.filter(supported.contains)
        return kept.isEmpty ? supported : kept
    }

    /// The library's transport schemes for these cipher names, in this order.
    static func schemes(_ ciphers: [String]) -> [NIOSSHTransportProtection.Type] {
        let bundled = NIOSSH.Constants.bundledTransportProtectionSchemes
        return ciphers.compactMap { name -> NIOSSHTransportProtection.Type? in
            bundled.first { $0.cipherName == name }
        }
    }

    static func nioKey(_ key: PhoneKey) -> NIOSSHPrivateKey {
        switch key.backing {
        case .secureEnclaveP256(let enclaveKey): return NIOSSHPrivateKey(secureEnclaveP256Key: enclaveKey)
        case .keychainEd25519(let edKey): return NIOSSHPrivateKey(ed25519Key: edKey)
        }
    }

    static func describe(_ error: Error) -> String {
        if let channelError = error as? ChannelError, case .connectTimeout = channelError { return "the connection timed out" }
        return "could not connect"
    }
}

/// Runs its action once, from whichever path ends a channel first.
final class EndOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    private let action: @Sendable () -> Void

    init(_ action: @escaping @Sendable () -> Void) {
        self.action = action
    }

    func run() {
        let first: Bool = lock.withLock {
            defer { done = true }
            return !done
        }
        if first { action() }
    }
}

/// Records the first error on the connection and closes it.
final class ConnectionErrors: ChannelInboundHandler {
    typealias InboundIn = Any

    private let connection: HostConnection

    init(_ connection: HostConnection) {
        self.connection = connection
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        connection.fail("ssh: \(connection.host.display): the connection failed")
        context.close(promise: nil)
    }
}
