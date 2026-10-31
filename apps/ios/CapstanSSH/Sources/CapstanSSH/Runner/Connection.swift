import CapstanPhoneCore
import Foundation
import NIOCore
import NIOSSH

/// One SSH connection to a host, shared by its exec channels. Its state is guarded by `lock`; NIO objects are only
/// touched on `loop`.
final class HostConnection: @unchecked Sendable {
    let host: PhoneHost
    let loop: EventLoop
    /// Succeeds with the TCP channel once it is connected (auth and channel opening come later). Completed only
    /// through `resolve`, so it is completed once.
    private let connected: EventLoopPromise<Channel>

    private let lock = NSLock()
    private var failureText: String?
    private var openChannels = 0
    private var idleTask: Task<Void, Never>?
    private var closed = false
    private var resolved = false

    init(host: PhoneHost, loop: EventLoop) {
        self.host = host
        self.loop = loop
        connected = loop.makePromise(of: Channel.self)
    }

    var channel: EventLoopFuture<Channel> {
        connected.futureResult
    }

    /// Completes the connect promise the first time; later results are dropped (a channel that connected after a
    /// close is closed at once).
    func resolve(_ result: Result<Channel, Error>) {
        let first: Bool = lock.withLock {
            defer { resolved = true }
            return !resolved
        }
        if first {
            connected.completeWith(result)
        } else if case .success(let late) = result {
            late.close(promise: nil)
        }
    }

    /// The first failure is the one channels report.
    func fail(_ text: String) {
        lock.withLock { if failureText == nil { failureText = text } }
    }

    var failure: String? {
        lock.withLock { failureText }
    }

    /// Counts a new channel and stops a pending idle close. False when the connection is already closed.
    func openChannel() -> Bool {
        lock.withLock {
            guard !closed else { return false }
            openChannels += 1
            idleTask?.cancel()
            idleTask = nil
            return true
        }
    }

    /// Counts a channel that ended. True when it was the last one.
    func closeChannel() -> Bool {
        lock.withLock {
            openChannels = max(0, openChannels - 1)
            return openChannels == 0 && !closed
        }
    }

    /// Replaces the idle-close task, unless a channel opened meanwhile.
    func setIdleTask(_ task: Task<Void, Never>) {
        let keep: Bool = lock.withLock {
            guard openChannels == 0, !closed else { return false }
            idleTask?.cancel()
            idleTask = task
            return true
        }
        if !keep { task.cancel() }
    }

    /// Still idle when the idle task wakes up.
    var isIdle: Bool {
        lock.withLock { openChannels == 0 && !closed }
    }

    /// Marks the connection closed and closes its TCP channel. Open channels then end with 255.
    func close(reason: String) {
        let task: Task<Void, Never>? = lock.withLock {
            if failureText == nil { failureText = reason }
            closed = true
            let pending = idleTask
            idleTask = nil
            return pending
        }
        task?.cancel()
        connected.futureResult.whenSuccess { $0.close(promise: nil) }
        resolve(.failure(SSHRunnerError.closed))
    }
}

enum SSHRunnerError: Error {
    case closed
    case hostKeyRejected
    case noKey
}

// MARK: Auth

/// Offers the vault's key once. A second question means the server refused it: no other method is ever tried.
final class VaultKeyAuth: NIOSSHClientUserAuthenticationDelegate {
    private let user: String
    private let key: NIOSSHPrivateKey
    private let refused: @Sendable () -> Void
    private var offered = false

    init(user: String, key: NIOSSHPrivateKey, refused: @escaping @Sendable () -> Void) {
        self.user = user
        self.key = key
        self.refused = refused
    }

    func nextAuthenticationType(
        availableMethods: NIOSSHAvailableUserAuthenticationMethods,
        nextChallengePromise: EventLoopPromise<NIOSSHUserAuthenticationOffer?>
    ) {
        guard !offered, availableMethods.contains(.publicKey) else {
            refused()
            nextChallengePromise.succeed(nil)
            return
        }
        offered = true
        nextChallengePromise.succeed(
            NIOSSHUserAuthenticationOffer(username: user, serviceName: "", offer: .privateKey(.init(privateKey: key))))
    }
}

/// Checks the host key against the pins, on the event loop (the verdict is read from memory).
final class PinnedHostKeys: NIOSSHClientServerAuthenticationDelegate {
    private let accept: @Sendable (HostKeyPin) -> Bool

    init(accept: @escaping @Sendable (HostKeyPin) -> Bool) {
        self.accept = accept
    }

    func validateHostKey(hostKey: NIOSSHPublicKey, validationCompletePromise: EventLoopPromise<Void>) {
        if let pin = HostKeyPin(openSSH: String(openSSHPublicKey: hostKey)), accept(pin) {
            validationCompletePromise.succeed(())
        } else {
            validationCompletePromise.fail(SSHRunnerError.hostKeyRejected)
        }
    }
}

extension HostKeyPin {
    /// From `<algorithm> <base64 blob>`, as `String(openSSHPublicKey:)` prints it.
    init?(openSSH text: String) {
        let parts = text.split(separator: " ")
        guard parts.count >= 2, let blob = Data(base64Encoded: String(parts[1])) else { return nil }
        self.init(algorithm: String(parts[0]), blob: blob)
    }
}

// MARK: The server's offer

/// The server's algorithm lists from its first key-exchange message.
struct ServerOffer: Sendable, Equatable {
    var kex: [String]
    var hostKeys: [String]
    var ciphers: [String]
}

/// Reads the server's version line and first packet (its KEXINIT, sent in plain text) as they pass, so a connection
/// with nothing in common can end with a reason. Every byte is passed on unchanged.
final class ServerOfferSniffer: ChannelInboundHandler {
    typealias InboundIn = ByteBuffer
    typealias InboundOut = ByteBuffer

    /// Called once with the offer; a reason closes the connection before the library reads the message.
    private let decide: @Sendable (ServerOffer) -> UnsupportedReason?
    private var buffer = ByteBuffer()
    private var versionSeen = false
    private var done = false

    init(decide: @escaping @Sendable (ServerOffer) -> UnsupportedReason?) {
        self.decide = decide
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        if !done {
            var bytes = unwrapInboundIn(data)
            buffer.writeBuffer(&bytes)
            if let offer = parse() {
                done = true
                buffer = ByteBuffer()
                if decide(offer) != nil {
                    context.close(promise: nil)
                    return
                }
            } else if buffer.readableBytes > 65_536 {
                // Not an SSH server we can read; the library will say so.
                done = true
                buffer = ByteBuffer()
            }
        }
        context.fireChannelRead(data)
    }

    private func parse() -> ServerOffer? {
        var view = buffer
        while !versionSeen {
            guard let end = view.readableBytesView.firstIndex(of: UInt8(ascii: "\n")) else { return nil }
            let length = end - view.readerIndex + 1
            guard let line = view.readString(length: length) else { return nil }
            if line.hasPrefix("SSH-") { versionSeen = true }
            buffer = view
        }
        guard let packetLength = view.readInteger(as: UInt32.self), packetLength >= 1, packetLength < 65_536,
              var body = view.readSlice(length: Int(packetLength)),
              let padding = body.readInteger(as: UInt8.self),
              body.readableBytes >= Int(padding),
              var payload = body.readSlice(length: body.readableBytes - Int(padding)),
              payload.readInteger(as: UInt8.self) == 20,
              payload.readSlice(length: 16) != nil,
              let kex = ServerOfferSniffer.nameList(&payload),
              let hostKeys = ServerOfferSniffer.nameList(&payload),
              let ciphers = ServerOfferSniffer.nameList(&payload)
        else { return nil }
        return ServerOffer(kex: kex, hostKeys: hostKeys, ciphers: ciphers)
    }

    private static func nameList(_ payload: inout ByteBuffer) -> [String]? {
        guard let length = payload.readInteger(as: UInt32.self), let text = payload.readString(length: Int(length))
        else { return nil }
        return text.isEmpty ? [] : text.split(separator: ",").map(String.init)
    }
}

extension ServerOffer {
    /// Why nothing can be agreed with this server under `preferences`, or nil. Preferences narrow the library's
    /// lists (names the library lacks are ignored; a list with none left means the library's own).
    func unsupported(_ preferences: AlgorithmPreferences, ciphers configured: [String]) -> UnsupportedReason? {
        func narrow(_ wanted: [String], _ supported: [String]) -> [String] {
            let kept = wanted.filter(supported.contains)
            return kept.isEmpty ? supported : kept
        }
        let hostKeyNames = narrow(preferences.hostKeys, SupportedAlgorithms.hostKeys)
        if !hostKeys.contains(where: hostKeyNames.contains) { return .hostKeyType(hostKeys.first ?? "") }
        let kexNames = narrow(preferences.kex, SupportedAlgorithms.kex)
        if !kex.contains(where: kexNames.contains) { return .noCommonAlgorithm }
        if !ciphers.contains(where: configured.contains) { return .noCommonAlgorithm }
        return nil
    }
}

// MARK: Exec channel

/// Runs one command: exec with a reply, stdin EOF once it started, output into the process, exit code at the end.
final class ExecHandler: ChannelInboundHandler {
    typealias InboundIn = SSHChannelData

    private let command: String
    private let process: SSHProcess
    private let connection: HostConnection
    private let started: @Sendable () -> Void
    private let ended: @Sendable () -> Void
    private var refusal: String?
    private var finished = false

    init(
        command: String, process: SSHProcess, connection: HostConnection, started: @escaping @Sendable () -> Void,
        ended: @escaping @Sendable () -> Void
    ) {
        self.command = command
        self.process = process
        self.connection = connection
        self.started = started
        self.ended = ended
    }

    func handlerAdded(context: ChannelHandlerContext) {
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).whenFailure { _ in }
    }

    func channelActive(context: ChannelHandlerContext) {
        context.triggerUserOutboundEvent(SSHChannelRequestEvent.ExecRequest(command: command, wantReply: true), promise: nil)
        context.fireChannelActive()
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let data = unwrapInboundIn(data)
        guard case .byteBuffer(let bytes) = data.data else { return }
        let chunk = Data(bytes.readableBytesView)
        switch data.type {
        case .channel: process.receiveStdout(chunk)
        case .stdErr: process.receiveStderr(chunk)
        default: break
        }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case is ChannelSuccessEvent:
            started()
            // The Mac runs ssh -T and never writes stdin: a command that reads it ends.
            context.close(mode: .output, promise: nil)
        case is ChannelFailureEvent:
            refusal = "ssh: \(connection.host.display): the host refused to run the command"
            context.close(promise: nil)
        case let status as SSHChannelRequestEvent.ExitStatus:
            process.receiveExit(Int32(clamping: status.exitStatus))
        case let signal as SSHChannelRequestEvent.ExitSignal:
            process.receiveExit(ExitSignals.code(for: signal.signalName))
        default:
            context.fireUserInboundEventTriggered(event)
        }
    }

    func channelInactive(context: ChannelHandlerContext) {
        end()
        context.fireChannelInactive()
    }

    func handlerRemoved(context: ChannelHandlerContext) {
        end()
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        context.close(promise: nil)
    }

    private func end() {
        guard !finished else { return }
        finished = true
        process.finish(failure: refusal ?? connection.failure ?? "ssh: \(connection.host.display): the channel closed")
        ended()
    }
}

/// Exit codes for `exit-signal`: 128 plus the signal number, with the numbers every POSIX host shares.
enum ExitSignals {
    static let numbers: [String: Int32] = [
        "HUP": 1, "INT": 2, "QUIT": 3, "ILL": 4, "TRAP": 5, "ABRT": 6, "FPE": 8, "KILL": 9, "SEGV": 11, "PIPE": 13,
        "ALRM": 14, "TERM": 15,
    ]

    /// 128 + n for a known signal; 255 for a name whose number differs between systems.
    static func code(for name: String) -> Int32 {
        let bare = name.hasPrefix("SIG") ? String(name.dropFirst(3)) : name
        guard let number = numbers[bare] else { return 255 }
        return 128 + number
    }
}
