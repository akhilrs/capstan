import NIOConcurrencyHelpers
import NIOCore
import NIOPosix

/// What the client sent to a probe server. The library's key-exchange list cannot be changed on either side, so a
/// probe offers one algorithm itself and reads the client's answer in plain text (before any encryption).
public struct KexProbeResult: Sendable, Equatable {
    /// The client's KEXINIT name-lists, in its order.
    public var keyExchange: [String] = []
    public var hostKeys: [String] = []
    public var ciphers: [String] = []
    /// The type of the client's next packet: 30 (KEX_ECDH_INIT) when it accepted the offer.
    public var nextMessage: UInt8?
    /// The length of the client's ephemeral public key in that packet.
    public var ephemeralKeyBytes: Int?
}

/// The expected ephemeral key length for each key exchange the app needs.
public let ephemeralKeyBytes: [String: Int] = [
    "curve25519-sha256": 32,
    "ecdh-sha2-nistp256": 65,
    "ecdh-sha2-nistp384": 97,
    "ecdh-sha2-nistp521": 133,
]

/// A fake SSH server on 127.0.0.1 that sends one KEXINIT and records the client's KEXINIT and next packet.
public final class KexProbeServer {
    private let channel: Channel
    private let result: EventLoopPromise<KexProbeResult>

    public var port: Int {
        channel.localAddress?.port ?? 0
    }

    /// The probe's offer: one key-exchange algorithm; every host key and cipher the library knows.
    public init(group: EventLoopGroup, keyExchange: String) throws {
        let loop = group.next()
        let result = loop.makePromise(of: KexProbeResult.self)
        self.result = result
        let taken = NIOLockedValueBox(false)
        let bootstrap = ServerBootstrap(group: loop)
            .childChannelInitializer { channel in
                channel.eventLoop.makeCompletedFuture {
                    // One connection per probe.
                    let first = taken.withLockedValue { value -> Bool in
                        defer { value = true }
                        return !value
                    }
                    guard first else { throw SpikeError.unexpected("second connection") }
                    try channel.pipeline.syncOperations.addHandler(
                        ProbeHandler(keyExchange: keyExchange, result: result))
                }
            }
        channel = try bootstrap.bind(host: loopbackHost, port: 0).wait()
    }

    /// The record, once the client sent its answer or closed the connection.
    public func wait(seconds: Int64 = 10) throws -> KexProbeResult {
        try waitFor(result.futureResult, seconds: seconds)
    }

    public func close() {
        try? channel.close().wait()
    }
}

final class ProbeHandler: ChannelInboundHandler {
    typealias InboundIn = ByteBuffer
    typealias OutboundOut = ByteBuffer

    private let keyExchange: String
    private let result: EventLoopPromise<KexProbeResult>
    private var buffer = ByteBuffer()
    private var versionSeen = false
    private var record = KexProbeResult()
    private var kexinitSeen = false
    private var finished = false

    init(keyExchange: String, result: EventLoopPromise<KexProbeResult>) {
        self.keyExchange = keyExchange
        self.result = result
    }

    func channelActive(context: ChannelHandlerContext) {
        context.writeAndFlush(wrapOutboundOut(context.channel.allocator.buffer(string: "SSH-2.0-SpikeProbe\r\n")), promise: nil)
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        var bytes = unwrapInboundIn(data)
        buffer.writeBuffer(&bytes)
        if !versionSeen {
            guard let end = buffer.readableBytesView.firstIndex(of: UInt8(ascii: "\n")) else { return }
            buffer.moveReaderIndex(to: end + 1)
            versionSeen = true
            context.writeAndFlush(wrapOutboundOut(packet(kexinit(), allocator: context.channel.allocator)), promise: nil)
        }
        while let payload = nextPayload() {
            handle(payload, context: context)
            if finished { return }
        }
    }

    func channelInactive(context: ChannelHandlerContext) {
        finish(context)
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        finish(context)
    }

    private func handle(_ payload: ByteBuffer, context: ChannelHandlerContext) {
        var payload = payload
        guard let type = payload.readInteger(as: UInt8.self) else { return }
        if !kexinitSeen {
            guard type == 20 else { return }
            kexinitSeen = true
            payload.moveReaderIndex(forwardBy: 16)
            record.keyExchange = readNameList(&payload) ?? []
            record.hostKeys = readNameList(&payload) ?? []
            record.ciphers = readNameList(&payload) ?? []
            return
        }
        record.nextMessage = type
        if type == 30, let length = payload.readInteger(as: UInt32.self) {
            record.ephemeralKeyBytes = Int(length)
        }
        finish(context)
    }

    private func finish(_ context: ChannelHandlerContext) {
        guard !finished else { return }
        finished = true
        result.succeed(record)
        context.close(promise: nil)
    }

    /// The payload of the next whole binary packet (RFC 4253 section 6, no encryption, no MAC).
    private func nextPayload() -> ByteBuffer? {
        guard let length = buffer.getInteger(at: buffer.readerIndex, as: UInt32.self),
              buffer.readableBytes >= 4 + Int(length), length >= 1
        else { return nil }
        buffer.moveReaderIndex(forwardBy: 4)
        guard var body = buffer.readSlice(length: Int(length)),
              let padding = body.readInteger(as: UInt8.self), body.readableBytes >= Int(padding)
        else { return nil }
        return body.readSlice(length: body.readableBytes - Int(padding))
    }

    private func readNameList(_ payload: inout ByteBuffer) -> [String]? {
        guard let length = payload.readInteger(as: UInt32.self),
              let text = payload.readString(length: Int(length))
        else { return nil }
        return text.isEmpty ? [] : text.split(separator: ",").map(String.init)
    }

    private func kexinit() -> ByteBuffer {
        var payload = ByteBuffer()
        payload.writeInteger(UInt8(20))
        payload.writeRepeatingByte(0x5A, count: 16)
        let ciphers = "aes256-gcm@openssh.com,aes128-gcm@openssh.com"
        let lists = [
            keyExchange,
            "ssh-ed25519,ecdsa-sha2-nistp256,ecdsa-sha2-nistp384,ecdsa-sha2-nistp521",
            ciphers, ciphers,
            "hmac-sha2-256", "hmac-sha2-256",
            "none", "none",
            "", "",
        ]
        for list in lists {
            payload.writeInteger(UInt32(list.utf8.count))
            payload.writeString(list)
        }
        payload.writeInteger(UInt8(0))
        payload.writeInteger(UInt32(0))
        return payload
    }

    private func packet(_ payload: ByteBuffer, allocator: ByteBufferAllocator) -> ByteBuffer {
        var padding = 8 - (5 + payload.readableBytes) % 8
        if padding < 4 { padding += 8 }
        var out = allocator.buffer(capacity: 5 + payload.readableBytes + padding)
        out.writeInteger(UInt32(1 + payload.readableBytes + padding))
        out.writeInteger(UInt8(padding))
        var payload = payload
        out.writeBuffer(&payload)
        out.writeRepeatingByte(0, count: padding)
        return out
    }
}
