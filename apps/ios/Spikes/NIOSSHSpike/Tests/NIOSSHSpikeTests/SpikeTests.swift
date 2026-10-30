import CryptoKit
import NIOCore
import NIOPosix
import NIOSSH
import NIOSSHSpike
import XCTest

/// One test per item of the ssh-spike list in docs/plans/iphone-app-plan.md (section 5.3). Every server and client
/// uses 127.0.0.1. Each test prints a `SPIKE item <n>:` line for the results table.
final class SpikeTests: XCTestCase {
    private var group: MultiThreadedEventLoopGroup!
    private let userKey = NIOSSHPrivateKey(ed25519Key: Curve25519.Signing.PrivateKey())
    private let hostKey = NIOSSHPrivateKey(ed25519Key: Curve25519.Signing.PrivateKey())

    override func setUp() {
        super.setUp()
        group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
    }

    override func tearDown() {
        try? group.syncShutdownGracefully()
        group = nil
        super.tearDown()
    }

    private func report(_ item: Int, _ text: String) {
        print("SPIKE item \(item): \(text)")
    }

    /// A server, a client and an open session channel, with the exec request sent and answered.
    private struct Session {
        let server: SpikeServer
        let client: SpikeClient
        let handler: ClientSessionHandler
        let channel: Channel

        func close() {
            client.close()
            server.close()
        }
    }

    private func startSession(
        command: String, userKey: NIOSSHPrivateKey? = nil, hostKeys: [NIOSSHPrivateKey]? = nil, cipher: String? = nil
    ) throws -> Session {
        let userKey = userKey ?? self.userKey
        let server = try SpikeServer(
            group: group, hostKeys: hostKeys ?? [hostKey], clientKey: userKey.publicKey, cipher: cipher)
        let client = try SpikeClient(group: group, port: server.port, key: userKey)
        let handler = ClientSessionHandler(eventLoop: client.channel.eventLoop)
        let channel = try waitFor(client.openSession(handler))
        try waitFor(channel.triggerUserOutboundEvent(SSHChannelRequestEvent.ExecRequest(command: command, wantReply: true)))
        XCTAssertTrue(try waitFor(handler.reply.futureResult), "exec was refused")
        return Session(server: server, client: client, handler: handler, channel: channel)
    }

    // 1. Client userauth signed with a Secure Enclave key.
    func testItem1SecureEnclaveUserAuth() throws {
        guard SecureEnclave.isAvailable else {
            report(1, "skipped (no Secure Enclave here)")
            throw XCTSkip("No Secure Enclave on this machine; device check 1 decides.")
        }
        let enclaveKey: SecureEnclave.P256.Signing.PrivateKey
        do {
            enclaveKey = try SecureEnclave.P256.Signing.PrivateKey()
        } catch {
            report(1, "skipped (could not create an enclave key: \(error))")
            throw XCTSkip("Could not create a Secure Enclave key in this test process: \(error)")
        }
        let key = NIOSSHPrivateKey(secureEnclaveP256Key: enclaveKey)
        let session = try startSession(command: SpikeCommand.dieBySignal, userKey: key)
        defer { session.close() }
        // The library verifies the signature before the server's delegate runs, so "auth accepted" means it was valid.
        XCTAssertTrue(session.server.recorder.contains("auth accepted"))
        report(1, "passed (ecdsa-sha2-nistp256 signature from the Secure Enclave accepted)")
    }

    // 2. An outbound signal request.
    func testItem2OutboundSignal() throws {
        let session = try startSession(command: SpikeCommand.waitForSignal)
        defer { session.close() }
        try waitFor(session.channel.triggerUserOutboundEvent(SSHChannelRequestEvent.SignalRequest(signal: "TERM")))
        try waitFor(session.channel.closeFuture)
        XCTAssertTrue(session.server.recorder.contains("signal TERM"), "\(session.server.recorder.all)")
        XCTAssertTrue(session.handler.recorder.contains("exit-status 0"), "\(session.handler.recorder.all)")
        report(2, "passed (server received signal TERM)")
    }

    // 3. An exit-signal received.
    func testItem3ExitSignalReceived() throws {
        let session = try startSession(command: SpikeCommand.dieBySignal)
        defer { session.close() }
        try waitFor(session.channel.closeFuture)
        XCTAssertTrue(session.handler.recorder.contains("exit-signal KILL"), "\(session.handler.recorder.all)")
        report(3, "passed (client received exit-signal KILL)")
    }

    // 4. Stdin EOF after exec; also stdout, stderr (extended data) and exit-status.
    func testItem4StdinEOFAfterExec() throws {
        let session = try startSession(command: SpikeCommand.echoAfterEOF)
        defer { session.close() }
        try waitFor(session.channel.close(mode: .output))
        try waitFor(session.channel.closeFuture)
        XCTAssertEqual(
            session.server.recorder.all.filter { !$0.hasPrefix("auth") }, ["exec \(SpikeCommand.echoAfterEOF)", "eof"])
        let received = session.handler.recorder.all
        XCTAssertTrue(received.contains("stdout out\n"), "\(received)")
        XCTAssertTrue(received.contains("stderr err\n"), "\(received)")
        XCTAssertTrue(received.contains("exit-status 3"), "\(received)")
        report(4, "passed (EOF after exec reached the server; stdout, stderr and exit-status 3 came back)")
    }

    // 5. Key exchange: each algorithm offered alone by a probe server; the client must answer with KEX_ECDH_INIT.
    func testItem5KeyExchanges() throws {
        var clientOrder: [String] = []
        for (algorithm, keyBytes) in ephemeralKeyBytes.sorted(by: { $0.key < $1.key }) {
            let probe = try KexProbeServer(group: group, keyExchange: algorithm)
            let client = try SpikeClient(group: group, port: probe.port, key: userKey)
            let result = try probe.wait()
            client.close()
            probe.close()
            clientOrder = result.keyExchange
            XCTAssertEqual(result.nextMessage, 30, "\(algorithm): the client did not answer with KEX_ECDH_INIT")
            XCTAssertEqual(result.ephemeralKeyBytes, keyBytes, "\(algorithm): wrong ephemeral key size")
            let passed = result.nextMessage == 30 && result.ephemeralKeyBytes == keyBytes
            report(5, "\(algorithm) \(passed ? "passed" : "failed")")
        }
        report(5, "client order: \(clientOrder.joined(separator: ","))")
    }

    // 6. Host keys: a full handshake with a server that has only one host key of each kind.
    func testItem6HostKeys() throws {
        let keys: [(String, NIOSSHPrivateKey)] = [
            ("ssh-ed25519", NIOSSHPrivateKey(ed25519Key: Curve25519.Signing.PrivateKey())),
            ("ecdsa-sha2-nistp256", NIOSSHPrivateKey(p256Key: P256.Signing.PrivateKey())),
            ("ecdsa-sha2-nistp384", NIOSSHPrivateKey(p384Key: P384.Signing.PrivateKey())),
            ("ecdsa-sha2-nistp521", NIOSSHPrivateKey(p521Key: P521.Signing.PrivateKey())),
        ]
        for (algorithm, key) in keys {
            let session = try startSession(command: SpikeCommand.dieBySignal, hostKeys: [key])
            let presented = session.client.hostKeys.all
            session.close()
            XCTAssertEqual(presented.count, 1, algorithm)
            let passed = presented.first?.hasPrefix(algorithm + " ") == true
            XCTAssertTrue(passed, "\(algorithm): presented \(presented)")
            report(6, "\(algorithm) \(passed ? "passed" : "failed")")
        }
    }

    // 7. Ciphers: a full handshake and a session with a server that allows only one cipher.
    func testItem7Ciphers() throws {
        for cipher in ["aes128-gcm@openssh.com", "aes256-gcm@openssh.com"] {
            XCTAssertEqual(transportSchemes(cipher: cipher).count, 1, "\(cipher) is not in the bundled list")
            let session = try startSession(command: SpikeCommand.echoAfterEOF, cipher: cipher)
            try waitFor(session.channel.close(mode: .output))
            try waitFor(session.channel.closeFuture)
            let passed = session.handler.recorder.contains("exit-status 3")
            session.close()
            XCTAssertTrue(passed, cipher)
            report(7, "\(cipher) \(passed ? "passed" : "failed")")
        }
    }

    // 8. Whether the client's host-key algorithm order can be set. SSHClientConfiguration has no field for it, so the
    // probe records the order the client sends; "fixed" is a result, not a failure.
    func testItem8HostKeyOrderProbe() throws {
        let probe = try KexProbeServer(group: group, keyExchange: "curve25519-sha256")
        let client = try SpikeClient(group: group, port: probe.port, key: userKey)
        let result = try probe.wait()
        client.close()
        probe.close()
        XCTAssertFalse(result.hostKeys.isEmpty)
        report(8, "fixed (no client setting); order: \(result.hostKeys.joined(separator: ","))")
    }
}
