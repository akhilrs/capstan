import CapstanPhoneCore
import CryptoKit
import Foundation
import NIOCore
import NIOPosix
import NIOSSH

/// What a test server saw.
final class ServerLog: @unchecked Sendable {
    private let lock = NSLock()
    private var connectionCount = 0
    private var disconnectCount = 0
    private var methods: [String] = []
    private var commands: [String] = []

    func connected() { lock.withLock { connectionCount += 1 } }
    func disconnected() { lock.withLock { disconnectCount += 1 } }
    func auth(_ method: String) { lock.withLock { methods.append(method) } }
    func exec(_ command: String) { lock.withLock { commands.append(command) } }

    var connections: Int { lock.withLock { connectionCount } }
    var disconnects: Int { lock.withLock { disconnectCount } }
    var authMethods: [String] { lock.withLock { methods } }
    var execs: [String] { lock.withLock { commands } }
}

/// An SSH server on 127.0.0.1 for the runner's tests. It accepts one public key and runs each exec request with
/// `/bin/sh -c` in a temporary folder whose `bin` comes first on PATH, so a test can put a fake `cstan` there.
/// `ssh-tuning`'s tests use it as it is.
final class TestSSHServer {
    let log = ServerLog()
    let hostKey: NIOSSHPrivateKey
    /// The folder the commands run in; `bin` inside it is first on PATH.
    let home: URL
    private let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
    private var channel: Channel!

    var port: Int { channel.localAddress?.port ?? 0 }

    /// The pin a client sees for this server.
    var hostPin: HostKeyPin {
        let text = String(openSSHPublicKey: hostKey.publicKey)
        let parts = text.split(separator: " ")
        return HostKeyPin(algorithm: String(parts[0]), blob: Data(base64Encoded: String(parts[1]))!)
    }

    init(acceptedKey: NIOSSHPublicKey?, hostKey: NIOSSHPrivateKey = NIOSSHPrivateKey(ed25519Key: .init())) throws {
        self.hostKey = hostKey
        home = FileManager.default.temporaryDirectory.appendingPathComponent("capstan-ssh-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home.appendingPathComponent("bin"), withIntermediateDirectories: true)
        let log = self.log
        let home = self.home
        let bootstrap = ServerBootstrap(group: group)
            .childChannelInitializer { channel in
                log.connected()
                channel.closeFuture.whenComplete { _ in log.disconnected() }
                return channel.eventLoop.makeCompletedFuture {
                    let configuration = SSHServerConfiguration(
                        hostKeys: [hostKey], userAuthDelegate: OneKeyAuth(key: acceptedKey, log: log))
                    try channel.pipeline.syncOperations.addHandler(
                        NIOSSHHandler(
                            role: .server(configuration), allocator: channel.allocator,
                            inboundChildChannelInitializer: { child, _ in
                                child.eventLoop.makeCompletedFuture {
                                    try child.pipeline.syncOperations.addHandler(ShellExecHandler(log: log, home: home))
                                }
                            }))
                }
            }
        channel = try bootstrap.bind(host: "127.0.0.1", port: 0).wait()
    }

    /// Writes an executable script to `bin`.
    func installScript(_ name: String, _ body: String) throws {
        let url = home.appendingPathComponent("bin").appendingPathComponent(name)
        try ("#!/bin/sh\n" + body).write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
    }

    func stop() {
        try? channel.close().wait()
        try? group.syncShutdownGracefully()
        try? FileManager.default.removeItem(at: home)
    }
}

/// Public-key auth with one key; every attempt is logged, so a test can see that no password was tried.
private final class OneKeyAuth: NIOSSHServerUserAuthenticationDelegate {
    private let key: NIOSSHPublicKey?
    private let log: ServerLog

    init(key: NIOSSHPublicKey?, log: ServerLog) {
        self.key = key
        self.log = log
    }

    var supportedAuthenticationMethods: NIOSSHAvailableUserAuthenticationMethods { [.publicKey, .password] }

    func requestReceived(
        request: NIOSSHUserAuthenticationRequest, responsePromise: EventLoopPromise<NIOSSHUserAuthenticationOutcome>
    ) {
        switch request.request {
        case .publicKey(let offer):
            log.auth("publickey")
            responsePromise.succeed(offer.publicKey == key ? .success : .failure)
        case .password:
            log.auth("password")
            responsePromise.succeed(.failure)
        default:
            log.auth("other")
            responsePromise.succeed(.failure)
        }
    }
}

/// Runs an exec request with /bin/sh and bridges its standard streams to the channel.
private final class ShellExecHandler: ChannelInboundHandler {
    typealias InboundIn = SSHChannelData
    typealias OutboundOut = SSHChannelData

    private let log: ServerLog
    private let home: URL
    private var process: Process?
    private var stdin: FileHandle?

    init(log: ServerLog, home: URL) {
        self.log = log
        self.home = home
    }

    func handlerAdded(context: ChannelHandlerContext) {
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).whenFailure { _ in }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case let exec as SSHChannelRequestEvent.ExecRequest:
            log.exec(exec.command)
            do {
                try run(exec.command, channel: context.channel)
                if exec.wantReply { context.triggerUserOutboundEvent(ChannelSuccessEvent(), promise: nil) }
            } catch {
                if exec.wantReply { context.triggerUserOutboundEvent(ChannelFailureEvent(), promise: nil) }
                context.close(promise: nil)
            }
        case let event as ChannelEvent where event == .inputClosed:
            try? stdin?.close()
            stdin = nil
        default:
            context.fireUserInboundEventTriggered(event)
        }
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let data = unwrapInboundIn(data)
        guard case .byteBuffer(let bytes) = data.data, data.type == .channel else { return }
        stdin?.write(Data(bytes.readableBytesView))
    }

    func channelInactive(context: ChannelHandlerContext) {
        if let process, process.isRunning { process.terminate() }
        context.fireChannelInactive()
    }

    private func run(_ command: String, channel: Channel) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        process.currentDirectoryURL = home
        process.environment = [
            "PATH": home.appendingPathComponent("bin").path + ":/usr/bin:/bin", "HOME": home.path, "LC_ALL": "C",
        ]
        let input = Pipe()
        let output = Pipe()
        let error = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = error

        let streams = DispatchGroup()
        for (pipe, type) in [(output, SSHChannelData.DataType.channel), (error, .stdErr)] {
            streams.enter()
            pipe.fileHandleForReading.readabilityHandler = { handle in
                let chunk = handle.availableData
                if chunk.isEmpty {
                    handle.readabilityHandler = nil
                    streams.leave()
                    return
                }
                channel.eventLoop.execute {
                    let buffer = channel.allocator.buffer(bytes: chunk)
                    channel.writeAndFlush(SSHChannelData(type: type, data: .byteBuffer(buffer)), promise: nil)
                }
            }
        }
        process.terminationHandler = { ended in
            // Output first, then the exit, then the close.
            streams.notify(queue: .global()) {
                channel.eventLoop.execute {
                    let event: Any
                    if ended.terminationReason == .uncaughtSignal {
                        event = SSHChannelRequestEvent.ExitSignal(
                            signalName: ShellExecHandler.signalName(ended.terminationStatus), errorMessage: "",
                            language: "", dumpedCore: false)
                    } else {
                        event = SSHChannelRequestEvent.ExitStatus(exitStatus: Int(ended.terminationStatus))
                    }
                    channel.triggerUserOutboundEvent(event).whenComplete { _ in channel.close(promise: nil) }
                }
            }
        }
        try process.run()
        self.process = process
        stdin = input.fileHandleForWriting
    }

    private static func signalName(_ number: Int32) -> String {
        [1: "HUP", 2: "INT", 3: "QUIT", 6: "ABRT", 9: "KILL", 13: "PIPE", 14: "ALRM", 15: "TERM"][number] ?? "USR1"
    }
}

/// A fake server that sends one KEXINIT with the given lists and then waits, for the unsupported-algorithm tests
/// (swift-nio-ssh cannot offer an RSA host key or a single key exchange).
final class FakeKexServer {
    let log = ServerLog()
    private let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
    private var channel: Channel!

    var port: Int { channel.localAddress?.port ?? 0 }

    init(kex: [String], hostKeys: [String], ciphers: [String] = ["aes128-gcm@openssh.com"]) throws {
        let log = self.log
        let bootstrap = ServerBootstrap(group: group)
            .childChannelInitializer { channel in
                log.connected()
                channel.closeFuture.whenComplete { _ in log.disconnected() }
                return channel.eventLoop.makeCompletedFuture {
                    try channel.pipeline.syncOperations.addHandler(
                        KexOfferHandler(lists: [kex, hostKeys, ciphers, ciphers]))
                }
            }
        channel = try bootstrap.bind(host: "127.0.0.1", port: 0).wait()
    }

    func stop() {
        try? channel.close().wait()
        try? group.syncShutdownGracefully()
    }
}

private final class KexOfferHandler: ChannelInboundHandler {
    typealias InboundIn = ByteBuffer
    typealias OutboundOut = ByteBuffer

    private let lists: [[String]]
    private var sent = false

    init(lists: [[String]]) {
        self.lists = lists
    }

    func channelActive(context: ChannelHandlerContext) {
        context.writeAndFlush(wrapOutboundOut(context.channel.allocator.buffer(string: "SSH-2.0-FakeKex\r\n")), promise: nil)
        context.fireChannelActive()
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        guard !sent else { return }
        sent = true
        var payload = ByteBuffer()
        payload.writeInteger(UInt8(20))
        payload.writeRepeatingByte(0x42, count: 16)
        for list in lists + [["hmac-sha2-256"], ["hmac-sha2-256"], ["none"], ["none"], [], []] {
            let text = list.joined(separator: ",")
            payload.writeInteger(UInt32(text.utf8.count))
            payload.writeString(text)
        }
        payload.writeInteger(UInt8(0))
        payload.writeInteger(UInt32(0))
        var padding = 8 - (5 + payload.readableBytes) % 8
        if padding < 4 { padding += 8 }
        var packet = context.channel.allocator.buffer(capacity: 5 + payload.readableBytes + padding)
        packet.writeInteger(UInt32(1 + payload.readableBytes + padding))
        packet.writeInteger(UInt8(padding))
        packet.writeBuffer(&payload)
        packet.writeRepeatingByte(0, count: padding)
        context.writeAndFlush(wrapOutboundOut(packet), promise: nil)
    }
}
