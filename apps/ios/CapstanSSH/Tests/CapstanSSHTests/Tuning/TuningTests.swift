import CapstanKit
import CapstanPhoneCore
import CryptoKit
import Foundation
import NIOCore
import NIOPosix
import NIOSSH
import XCTest
@testable import CapstanSSH

/// A runner with `TuningHook` and one host, for a server the test makes from the phone's public key.
private final class TuningFixture {
    let directory: URL
    let pins: HostKeyStore
    let registry: PhoneHostRegistry
    let clock = TestClock()
    let runner: SSHRunner
    let host: PhoneHost
    let config: HostConfig

    /// `server` gets the phone's public key and returns its port and the pin to trust.
    init(server: (NIOSSHPublicKey) throws -> (port: Int, pin: HostKeyPin)) throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("capstan-tuning-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        pins = HostKeyStore(directory: directory)
        registry = PhoneHostRegistry(store: pins)
        let vault = KeychainKeyVault(storage: MemoryKeyStorage())
        let key = try vault.create(.keychainEd25519)
        let (port, pin) = try server(SSHRunner.nioKey(key).publicKey)
        host = PhoneHost(id: registry.newHostId(), hostname: "127.0.0.1", port: port, user: "tester")
        try registry.register(host)
        pins.trust(hostId: host.id, pin: pin)
        config = HostConfig(id: host.id, displayName: "Test", destination: host.alias)
        runner = SSHRunner(
            registry: registry, vault: vault, trust: pins, tuning: TransportTuning(), clock: clock, hook: TuningHook())
    }

    func start(_ command: String) throws -> any RunningProcess {
        try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), RemoteCommand.sshArguments(config, remote: command))
    }

    func stop() {
        runner.shutdown()
        try? FileManager.default.removeItem(at: directory)
    }
}

final class TuningTests: XCTestCase {
    // MARK: Algorithms

    func testTheAlgorithmListMatchesTheSpike() {
        let preferences = TuningHook().algorithms(pinnedHostKey: nil)
        XCTAssertEqual(Set(preferences.kex), Set(["curve25519-sha256", "ecdh-sha2-nistp256", "ecdh-sha2-nistp384", "ecdh-sha2-nistp521"]))
        XCTAssertEqual(
            Set(preferences.hostKeys),
            Set(["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521"]))
        XCTAssertEqual(preferences.ciphers, ["aes128-gcm@openssh.com", "aes256-gcm@openssh.com"])
        for name in preferences.kex { XCTAssertTrue(SupportedAlgorithms.kex.contains(name), name) }
        for name in preferences.hostKeys { XCTAssertTrue(SupportedAlgorithms.hostKeys.contains(name), name) }
        for name in preferences.ciphers { XCTAssertTrue(SupportedAlgorithms.ciphers.contains(name), name) }
    }

    func testThePinnedHostKeyComesFirst() {
        XCTAssertEqual(TuningHook().algorithms(pinnedHostKey: "ecdsa-sha2-nistp256").hostKeys.first, "ecdsa-sha2-nistp256")
        XCTAssertEqual(TuningHook().algorithms(pinnedHostKey: "ssh-rsa").hostKeys.first, "ssh-ed25519", "unknown pins change nothing")
        XCTAssertEqual(TuningHook().algorithms(pinnedHostKey: nil).hostKeys.count, 4)
    }

    func testAServerWithOnlyANistp521HostKey() async throws {
        var server: TestSSHServer?
        let fixture = try TuningFixture { phoneKey in
            let made = try TestSSHServer(acceptedKey: phoneKey, hostKey: NIOSSHPrivateKey(p521Key: P521.Signing.PrivateKey()))
            server = made
            return (made.port, made.hostPin)
        }
        defer {
            fixture.stop()
            server?.stop()
        }
        XCTAssertEqual(server?.hostPin.algorithm, "ecdsa-sha2-nistp521")
        let process = try fixture.start("echo ok")
        let result = await collect(process)
        XCTAssertEqual(result.lines, ["ok"])
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
    }

    /// swift-nio-ssh 0.15.1 offers its host keys in a fixed order (spike item 8), so a pin cannot pull its type first.
    /// A host that also has an ed25519 key presents it, and a pin of another type then reads as changed (the safe side).
    /// The test server holds one key, so this shows the outcome: an ed25519 key against a P-256 pin.
    func testAPinOfAnotherTypeThanTheClientsFirstChoiceReadsAsChanged() async throws {
        var server: TestSSHServer?
        var p256Pin: HostKeyPin?
        let fixture = try TuningFixture { phoneKey in
            let p256 = NIOSSHPrivateKey(p256Key: P256.Signing.PrivateKey())
            let made = try TestSSHServer(acceptedKey: phoneKey, hostKey: NIOSSHPrivateKey(ed25519Key: .init()))
            server = made
            let parts = String(openSSHPublicKey: p256.publicKey).split(separator: " ")
            let pin = HostKeyPin(algorithm: String(parts[0]), blob: Data(base64Encoded: String(parts[1]))!)
            p256Pin = pin
            return (made.port, pin)
        }
        defer {
            fixture.stop()
            server?.stop()
        }
        XCTAssertEqual(p256Pin?.algorithm, "ecdsa-sha2-nistp256")
        let events = EventRecorder(fixture.runner.events)
        let process = try fixture.start("echo ok")
        let code = await exitCode(process)
        XCTAssertEqual(code, 255)
        let seen = await events.wait(for: 1)
        XCTAssertEqual(seen, [.hostKeyChanged(hostId: fixture.host.id)])
    }

    // MARK: terminate

    private func makeSignalFixture() throws -> (TuningFixture, SignalServer) {
        var server: SignalServer?
        let fixture = try TuningFixture { phoneKey in
            let made = try SignalServer(acceptedKey: phoneKey)
            server = made
            return (made.port, made.hostPin)
        }
        return (fixture, server!)
    }

    private func waitForExec(_ server: SignalServer) async {
        await eventually(timeout: 5, "the exec request") { !server.log.execs.isEmpty }
    }

    func testARemoteThatExitsOnTermEndsBeforeTheGraceWith143() async throws {
        let (fixture, server) = try makeSignalFixture()
        defer {
            fixture.stop()
            server.stop()
        }
        let process = try fixture.start("exit-on-term")
        await waitForExec(server)
        process.terminate(grace: 3)
        let code = await exitCode(process)
        XCTAssertEqual(code, 128 + 15)
        XCTAssertEqual(server.signals.all, ["TERM"])
    }

    func testARemoteThatIgnoresTermIsClosedAfterTheGrace() async throws {
        let (fixture, server) = try makeSignalFixture()
        defer {
            fixture.stop()
            server.stop()
        }
        let process = try fixture.start("ignore-term")
        await waitForExec(server)
        process.terminate(grace: 3)
        await eventually(timeout: 5, "the signal") { server.signals.all == ["TERM"] }
        await eventually(timeout: 5, "the grace timer") { fixture.clock.sleeperCount >= 1 }
        fixture.clock.advance(by: 2)
        await settle()
        let early = await exitCodeIfAny(process, within: 0.3)
        XCTAssertNil(early, "still running inside the grace")

        fixture.clock.advance(by: 1)
        let code = await exitCode(process)
        XCTAssertEqual(code, 255, "closed by the client, no exit status")
    }

    func testAnExitStatusAfterTheTerminateRequestIsTheExitCode() async throws {
        let (fixture, server) = try makeSignalFixture()
        defer {
            fixture.stop()
            server.stop()
        }
        let process = try fixture.start("status-on-term")
        await waitForExec(server)
        process.terminate(grace: 3)
        let code = await exitCode(process)
        XCTAssertEqual(code, 7)
    }

    // MARK: Socket options

    func testKeepaliveOptions() {
        let options = TuningHook(tuning: TransportTuning(keepaliveIdle: 15, keepaliveInterval: 15, keepaliveCount: 3))
            .channelOptions()
        XCTAssertEqual(options, [
            CapstanSSH.SocketOption(level: SOL_SOCKET, name: SO_KEEPALIVE, value: 1),
            CapstanSSH.SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPALIVE, value: 15),
            CapstanSSH.SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPINTVL, value: 15),
            CapstanSSH.SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPCNT, value: 3),
        ])
    }

    func testTheRunnerAppliesTheKeepaliveOptions() async throws {
        var server: TestSSHServer?
        let fixture = try TuningFixture { phoneKey in
            let made = try TestSSHServer(acceptedKey: phoneKey)
            server = made
            return (made.port, made.hostPin)
        }
        defer {
            fixture.stop()
            server?.stop()
        }
        let seen = SeenSocketOptions()
        fixture.runner.observeSocketOptions { seen.set($0) }
        let process = try fixture.start("echo ok")
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
        XCTAssertEqual(seen.value, TuningHook().channelOptions())
    }

    /// The options read back from a real socket: Darwin accepts them and keeps the values.
    func testTheOptionsReadBackFromASocket() throws {
        let server = try TestSSHServer(acceptedKey: nil)
        defer { server.stop() }
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        defer { try? group.syncShutdownGracefully() }
        let options = TuningHook().channelOptions()
        var bootstrap = ClientBootstrap(group: group)
        for option in options {
            bootstrap = bootstrap.channelOption(
                ChannelOptions.socket(SocketOptionLevel(option.level), SocketOptionName(option.name)),
                value: SocketOptionValue(option.value))
        }
        let channel = try bootstrap.connect(host: "127.0.0.1", port: server.port).wait()
        defer { try? channel.close().wait() }
        for option in options {
            let value = try channel.getOption(
                ChannelOptions.socket(SocketOptionLevel(option.level), SocketOptionName(option.name))).wait()
            if option.name == SO_KEEPALIVE {
                XCTAssertNotEqual(value, 0, "SO_KEEPALIVE is on")
            } else {
                XCTAssertEqual(value, option.value, "option \(option.name)")
            }
        }
    }
}

private final class SeenSocketOptions: @unchecked Sendable {
    private let lock = NSLock()
    private var options: [CapstanSSH.SocketOption]?

    func set(_ value: [CapstanSSH.SocketOption]) { lock.withLock { options = value } }
    var value: [CapstanSSH.SocketOption]? { lock.withLock { options } }
}

/// The exit code if it arrives within `seconds`, without failing the test when it does not.
private func exitCodeIfAny(_ process: any RunningProcess, within seconds: TimeInterval) async -> Int32? {
    let first = FirstResult<Int32?>()
    return await withCheckedContinuation { continuation in
        first.wait(continuation)
        Task { first.offer(await process.exitCode()) }
        Task {
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            first.offer(nil)
        }
    }
}
