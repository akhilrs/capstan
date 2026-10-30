import NIOConcurrencyHelpers
import NIOCore
import NIOPosix
import NIOSSH

/// The only address the spike listens on or connects to.
public let loopbackHost = "127.0.0.1"

public enum SpikeError: Error, Equatable {
    case timeout
    case unexpected(String)
}

/// A list of things that happened, safe to read from the test thread.
public final class Recorder: @unchecked Sendable {
    private let lock = NIOLock()
    private var items: [String] = []

    public init() {}

    public func add(_ item: String) {
        lock.withLock { items.append(item) }
    }

    public var all: [String] {
        lock.withLock { items }
    }

    public func contains(_ item: String) -> Bool {
        all.contains(item)
    }
}

/// Waits for a future, failing with `SpikeError.timeout` after `seconds`.
public func waitFor<T>(_ future: EventLoopFuture<T>, seconds: Int64 = 10) throws -> T {
    let loop = future.eventLoop
    let promise = loop.makePromise(of: T.self)
    let done = NIOLockedValueBox(false)
    func complete(_ result: Result<T, Error>) {
        let first = done.withLockedValue { value -> Bool in
            defer { value = true }
            return !value
        }
        if first { promise.completeWith(result) }
    }
    let timer = loop.scheduleTask(in: .seconds(seconds)) { complete(.failure(SpikeError.timeout)) }
    future.whenComplete { result in
        timer.cancel()
        complete(result)
    }
    return try promise.futureResult.wait()
}

/// The library's bundled schemes.
public func allTransportSchemes() -> [NIOSSHTransportProtection.Type] {
    NIOSSH.Constants.bundledTransportProtectionSchemes.map { $0 }
}

/// The schemes from the library's bundled list whose cipher is `name`.
public func transportSchemes(cipher name: String) -> [NIOSSHTransportProtection.Type] {
    NIOSSH.Constants.bundledTransportProtectionSchemes.filter { $0.cipherName == name }.map { $0 }
}

// MARK: Server

/// What the server's session channel does after an exec request.
public enum SpikeCommand {
    /// After the client's EOF: "out\n" on stdout, "err\n" on stderr, then exit status 3.
    public static let echoAfterEOF = "echo-after-eof"
    /// Waits for a signal request, records it, then exit status 0.
    public static let waitForSignal = "wait-for-signal"
    /// Ends at once with exit-signal KILL.
    public static let dieBySignal = "die-by-signal"
}

/// An SSH server on 127.0.0.1 that accepts one public key and runs the `SpikeCommand`s.
public final class SpikeServer {
    public let recorder: Recorder
    private let channel: Channel

    public var port: Int {
        channel.localAddress?.port ?? 0
    }

    public init(
        group: EventLoopGroup, hostKeys: [NIOSSHPrivateKey], clientKey: NIOSSHPublicKey,
        cipher: String? = nil
    ) throws {
        let recorder = Recorder()
        self.recorder = recorder
        let bootstrap = ServerBootstrap(group: group)
            .childChannelInitializer { channel in
                channel.eventLoop.makeCompletedFuture {
                    let configuration = SSHServerConfiguration(
                        hostKeys: hostKeys, userAuthDelegate: OneKeyServerAuth(key: clientKey, recorder: recorder),
                        globalRequestDelegate: nil, banner: nil,
                        transportProtectionSchemes: cipher.map(transportSchemes(cipher:)) ?? allTransportSchemes())
                    let handler = NIOSSHHandler(
                        role: .server(configuration), allocator: channel.allocator,
                        inboundChildChannelInitializer: { child, type in
                            guard case .session = type else {
                                return child.eventLoop.makeFailedFuture(SpikeError.unexpected("channel type"))
                            }
                            return child.eventLoop.makeCompletedFuture {
                                try child.pipeline.syncOperations.addHandler(ServerSessionHandler(recorder: recorder))
                            }
                        })
                    try channel.pipeline.syncOperations.addHandler(handler)
                }
            }
        channel = try bootstrap.bind(host: loopbackHost, port: 0).wait()
    }

    public func close() {
        try? channel.close().wait()
    }
}

/// Accepts public-key auth with one key. The library checks the signature before it asks this delegate.
final class OneKeyServerAuth: NIOSSHServerUserAuthenticationDelegate {
    private let key: NIOSSHPublicKey
    private let recorder: Recorder

    init(key: NIOSSHPublicKey, recorder: Recorder) {
        self.key = key
        self.recorder = recorder
    }

    var supportedAuthenticationMethods: NIOSSHAvailableUserAuthenticationMethods {
        .publicKey
    }

    func requestReceived(
        request: NIOSSHUserAuthenticationRequest, responsePromise: EventLoopPromise<NIOSSHUserAuthenticationOutcome>
    ) {
        guard case .publicKey(let offer) = request.request, offer.publicKey == key else {
            recorder.add("auth refused")
            responsePromise.succeed(.failure)
            return
        }
        recorder.add("auth accepted")
        responsePromise.succeed(.success)
    }
}

final class ServerSessionHandler: ChannelInboundHandler {
    typealias InboundIn = SSHChannelData
    typealias OutboundOut = SSHChannelData

    private let recorder: Recorder
    private var command: String?

    init(recorder: Recorder) {
        self.recorder = recorder
    }

    func handlerAdded(context: ChannelHandlerContext) {
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).assumeIsolated().whenFailure {
            error in
            context.fireErrorCaught(error)
        }
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let data = unwrapInboundIn(data)
        if case .byteBuffer(let bytes) = data.data {
            recorder.add("server read \(bytes.readableBytes) bytes")
        }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case let exec as SSHChannelRequestEvent.ExecRequest:
            command = exec.command
            recorder.add("exec \(exec.command)")
            if exec.wantReply { context.triggerUserOutboundEvent(ChannelSuccessEvent(), promise: nil) }
            if exec.command == SpikeCommand.dieBySignal {
                let signal = SSHChannelRequestEvent.ExitSignal(
                    signalName: "KILL", errorMessage: "", language: "", dumpedCore: false)
                finish(context, with: signal)
            }
        case let signal as SSHChannelRequestEvent.SignalRequest:
            recorder.add("signal \(signal.signal)")
            if command == SpikeCommand.waitForSignal {
                finish(context, with: SSHChannelRequestEvent.ExitStatus(exitStatus: 0))
            }
        case let channelEvent as ChannelEvent where channelEvent == .inputClosed:
            recorder.add("eof")
            if command == SpikeCommand.echoAfterEOF {
                write(context, "out\n", type: .channel)
                write(context, "err\n", type: .stdErr)
                finish(context, with: SSHChannelRequestEvent.ExitStatus(exitStatus: 3))
            }
        default:
            context.fireUserInboundEventTriggered(event)
        }
    }

    private func write(_ context: ChannelHandlerContext, _ text: String, type: SSHChannelData.DataType) {
        let buffer = context.channel.allocator.buffer(string: text)
        context.writeAndFlush(wrapOutboundOut(SSHChannelData(type: type, data: .byteBuffer(buffer))), promise: nil)
    }

    private func finish(_ context: ChannelHandlerContext, with event: Any) {
        let channel = context.channel
        context.triggerUserOutboundEvent(event).whenComplete { _ in
            channel.close(promise: nil)
        }
    }
}

// MARK: Client

/// An SSH client connection to 127.0.0.1 that authenticates with one private key.
public final class SpikeClient {
    /// The OpenSSH text of every host key the server presented (`<algorithm> <base64>`).
    public let hostKeys: Recorder
    public let channel: Channel

    public init(group: EventLoopGroup, port: Int, key: NIOSSHPrivateKey) throws {
        let hostKeys = Recorder()
        self.hostKeys = hostKeys
        let bootstrap = ClientBootstrap(group: group)
            .channelInitializer { channel in
                channel.eventLoop.makeCompletedFuture {
                    let configuration = SSHClientConfiguration(
                        userAuthDelegate: OneKeyClientAuth(key: key),
                        serverAuthDelegate: RecordingHostKeys(recorder: hostKeys))
                    try channel.pipeline.syncOperations.addHandler(
                        NIOSSHHandler(
                            role: .client(configuration), allocator: channel.allocator,
                            inboundChildChannelInitializer: nil))
                }
            }
        channel = try bootstrap.connect(host: loopbackHost, port: port).wait()
    }

    /// Opens a session channel once the handshake and user auth are done. Fails if either fails.
    public func openSession(_ handler: ClientSessionHandler) -> EventLoopFuture<Channel> {
        let channel = self.channel
        return channel.eventLoop.flatSubmit {
            let promise = channel.eventLoop.makePromise(of: Channel.self)
            do {
                let ssh = try channel.pipeline.syncOperations.handler(type: NIOSSHHandler.self)
                ssh.createChannel(promise, channelType: .session) { child, _ in
                    child.eventLoop.makeCompletedFuture {
                        try child.pipeline.syncOperations.addHandler(handler)
                    }
                }
            } catch {
                promise.fail(error)
            }
            return promise.futureResult
        }
    }

    public func close() {
        try? channel.close().wait()
    }
}

final class OneKeyClientAuth: NIOSSHClientUserAuthenticationDelegate {
    private let key: NIOSSHPrivateKey
    private var offered = false

    init(key: NIOSSHPrivateKey) {
        self.key = key
    }

    func nextAuthenticationType(
        availableMethods: NIOSSHAvailableUserAuthenticationMethods,
        nextChallengePromise: EventLoopPromise<NIOSSHUserAuthenticationOffer?>
    ) {
        guard !offered, availableMethods.contains(.publicKey) else {
            nextChallengePromise.succeed(nil)
            return
        }
        offered = true
        nextChallengePromise.succeed(
            NIOSSHUserAuthenticationOffer(username: "spike", serviceName: "", offer: .privateKey(.init(privateKey: key))))
    }
}

final class RecordingHostKeys: NIOSSHClientServerAuthenticationDelegate {
    private let recorder: Recorder

    init(recorder: Recorder) {
        self.recorder = recorder
    }

    func validateHostKey(hostKey: NIOSSHPublicKey, validationCompletePromise: EventLoopPromise<Void>) {
        recorder.add(String(openSSHPublicKey: hostKey))
        validationCompletePromise.succeed(())
    }
}

/// Records what arrives on a client session channel: data, request replies, exit status and exit signal.
public final class ClientSessionHandler: ChannelInboundHandler {
    public typealias InboundIn = SSHChannelData

    public let recorder = Recorder()
    /// Succeeds with true on channel-success, false on channel-failure.
    public let reply: EventLoopPromise<Bool>
    private var replied = false

    public init(eventLoop: EventLoop) {
        reply = eventLoop.makePromise(of: Bool.self)
    }

    public func handlerAdded(context: ChannelHandlerContext) {
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).assumeIsolated().whenFailure {
            error in
            context.fireErrorCaught(error)
        }
    }

    public func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let data = unwrapInboundIn(data)
        guard case .byteBuffer(let bytes) = data.data else { return }
        let text = String(buffer: bytes)
        switch data.type {
        case .channel: recorder.add("stdout \(text)")
        case .stdErr: recorder.add("stderr \(text)")
        default: recorder.add("extended \(text)")
        }
    }

    public func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case is ChannelSuccessEvent:
            answer(.success(true))
        case is ChannelFailureEvent:
            answer(.success(false))
        case let status as SSHChannelRequestEvent.ExitStatus:
            recorder.add("exit-status \(status.exitStatus)")
        case let signal as SSHChannelRequestEvent.ExitSignal:
            recorder.add("exit-signal \(signal.signalName)")
        default:
            context.fireUserInboundEventTriggered(event)
        }
    }

    public func handlerRemoved(context: ChannelHandlerContext) {
        answer(.failure(SpikeError.unexpected("channel closed before a reply")))
    }

    private func answer(_ result: Result<Bool, Error>) {
        guard !replied else { return }
        replied = true
        reply.completeWith(result)
    }
}
