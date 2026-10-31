import CapstanPhoneCore
import CryptoKit
import Foundation
import NIOCore
import NIOPosix
import NIOSSH

/// A scripted SSH server on 127.0.0.1 for the terminate tests. `TestSSHServer` runs real commands but does not pass
/// signal requests on, and the runner's fixtures are not edited here, so these commands are scripted:
/// - `exit-on-term`: ends with exit-signal TERM when it gets signal TERM.
/// - `status-on-term`: ends with exit-status 7 when it gets signal TERM.
/// - `ignore-term`: notes the signal and keeps running until the client closes the channel.
final class SignalServer {
    let log = ServerLog()
    let signals = Signals()
    let hostKey = NIOSSHPrivateKey(ed25519Key: .init())
    private let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
    private var channel: Channel!

    var port: Int { channel.localAddress?.port ?? 0 }

    var hostPin: HostKeyPin {
        let parts = String(openSSHPublicKey: hostKey.publicKey).split(separator: " ")
        return HostKeyPin(algorithm: String(parts[0]), blob: Data(base64Encoded: String(parts[1]))!)
    }

    final class Signals: @unchecked Sendable {
        private let lock = NSLock()
        private var names: [String] = []

        func add(_ name: String) { lock.withLock { names.append(name) } }
        var all: [String] { lock.withLock { names } }
    }

    init(acceptedKey: NIOSSHPublicKey) throws {
        let log = self.log
        let signals = self.signals
        let hostKey = self.hostKey
        let bootstrap = ServerBootstrap(group: group)
            .childChannelInitializer { channel in
                log.connected()
                return channel.eventLoop.makeCompletedFuture {
                    let configuration = SSHServerConfiguration(
                        hostKeys: [hostKey], userAuthDelegate: AcceptKey(key: acceptedKey))
                    try channel.pipeline.syncOperations.addHandler(
                        NIOSSHHandler(
                            role: .server(configuration), allocator: channel.allocator,
                            inboundChildChannelInitializer: { child, _ in
                                child.eventLoop.makeCompletedFuture {
                                    try child.pipeline.syncOperations.addHandler(
                                        ScriptedHandler(log: log, signals: signals))
                                }
                            }))
                }
            }
        channel = try bootstrap.bind(host: "127.0.0.1", port: 0).wait()
    }

    func stop() {
        try? channel.close().wait()
        try? group.syncShutdownGracefully()
    }
}

private final class AcceptKey: NIOSSHServerUserAuthenticationDelegate {
    private let key: NIOSSHPublicKey

    init(key: NIOSSHPublicKey) { self.key = key }

    var supportedAuthenticationMethods: NIOSSHAvailableUserAuthenticationMethods { .publicKey }

    func requestReceived(
        request: NIOSSHUserAuthenticationRequest, responsePromise: EventLoopPromise<NIOSSHUserAuthenticationOutcome>
    ) {
        guard case .publicKey(let offer) = request.request else { return responsePromise.succeed(.failure) }
        responsePromise.succeed(offer.publicKey == key ? .success : .failure)
    }
}

private final class ScriptedHandler: ChannelInboundHandler {
    typealias InboundIn = SSHChannelData

    private let log: ServerLog
    private let signals: SignalServer.Signals
    private var command = ""

    init(log: ServerLog, signals: SignalServer.Signals) {
        self.log = log
        self.signals = signals
    }

    func handlerAdded(context: ChannelHandlerContext) {
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).whenFailure { _ in }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case let exec as SSHChannelRequestEvent.ExecRequest:
            command = exec.command
            log.exec(exec.command)
            if exec.wantReply { context.triggerUserOutboundEvent(ChannelSuccessEvent(), promise: nil) }
        case let signal as SSHChannelRequestEvent.SignalRequest:
            signals.add(signal.signal)
            let channel = context.channel
            switch command {
            case "exit-on-term":
                let exit = SSHChannelRequestEvent.ExitSignal(
                    signalName: signal.signal, errorMessage: "", language: "", dumpedCore: false)
                context.triggerUserOutboundEvent(exit).whenComplete { _ in channel.close(promise: nil) }
            case "status-on-term":
                context.triggerUserOutboundEvent(SSHChannelRequestEvent.ExitStatus(exitStatus: 7))
                    .whenComplete { _ in channel.close(promise: nil) }
            default:
                break
            }
        default:
            context.fireUserInboundEventTriggered(event)
        }
    }
}
