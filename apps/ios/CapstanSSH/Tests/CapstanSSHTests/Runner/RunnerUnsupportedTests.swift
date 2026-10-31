import CapstanKit
import CapstanPhoneCore
import Foundation
import NIOCore
import XCTest
@testable import CapstanSSH

/// A server with nothing in common ends with `unsupported` and 255. swift-nio-ssh cannot be such a server, so these
/// use `FakeKexServer`, which only sends its algorithm lists.
final class RunnerUnsupportedTests: XCTestCase {
    private var fixture: RunnerFixture?
    private var fake: FakeKexServer?

    override func tearDown() {
        fixture?.stop()
        fake?.stop()
    }

    private func run(
        kex: [String], hostKeys: [String], hook: any TransportHook = DefaultHook()
    ) async throws -> (code: Int32?, tail: String, events: [TransportEvent], hostId: UUID) {
        let fake = try FakeKexServer(kex: kex, hostKeys: hostKeys)
        self.fake = fake
        let fixture = try RunnerFixture(trusted: false, hook: hook, port: fake.port)
        self.fixture = fixture
        let events = EventRecorder(fixture.runner.events)
        let process = try fixture.start("echo ran")
        let code = await exitCode(process)
        return (code, process.stderrTail(), await events.wait(for: 1), fixture.host.id)
    }

    func testOnlyAnRSAHostKey() async throws {
        let result = try await run(kex: ["curve25519-sha256"], hostKeys: ["ssh-rsa", "rsa-sha2-512"])
        XCTAssertEqual(result.code, 255)
        XCTAssertEqual(result.tail, "unsupported: host key type ssh-rsa\n")
        XCTAssertEqual(result.events, [.unsupported(hostId: result.hostId, .hostKeyType("ssh-rsa"))])
    }

    func testAHookKexRestrictionWithNothingInCommon() async throws {
        let result = try await run(
            kex: ["ecdh-sha2-nistp384"], hostKeys: ["ssh-ed25519"], hook: KexHook(kex: ["curve25519-sha256"]))
        XCTAssertEqual(result.code, 255)
        XCTAssertEqual(result.tail, "unsupported: no common algorithm\n")
        XCTAssertEqual(result.events, [.unsupported(hostId: result.hostId, .noCommonAlgorithm)])
    }

    func testAKexTheLibraryLacks() async throws {
        let result = try await run(kex: ["diffie-hellman-group14-sha256"], hostKeys: ["ssh-ed25519"])
        XCTAssertEqual(result.code, 255)
        XCTAssertEqual(result.events, [.unsupported(hostId: result.hostId, .noCommonAlgorithm)])
    }

    func testOfferChecks() {
        let offer = ServerOffer(kex: ["curve25519-sha256"], hostKeys: ["ssh-ed25519"], ciphers: ["aes128-gcm@openssh.com"])
        XCTAssertNil(offer.unsupported(SupportedAlgorithms.library, ciphers: SupportedAlgorithms.ciphers))
        XCTAssertEqual(
            offer.unsupported(SupportedAlgorithms.library, ciphers: ["aes256-gcm@openssh.com"]), .noCommonAlgorithm)
        let narrowed = AlgorithmPreferences(hostKeys: ["ecdsa-sha2-nistp256"], kex: [], ciphers: [])
        XCTAssertEqual(offer.unsupported(narrowed, ciphers: SupportedAlgorithms.ciphers), .hostKeyType("ssh-ed25519"))
        let unknownNames = AlgorithmPreferences(hostKeys: ["ssh-dss"], kex: ["sntrup761x25519-sha512"], ciphers: [])
        XCTAssertNil(offer.unsupported(unknownNames, ciphers: SupportedAlgorithms.ciphers), "unknown names are ignored")
    }

    func testExitSignalCodes() {
        XCTAssertEqual(ExitSignals.code(for: "TERM"), 143)
        XCTAssertEqual(ExitSignals.code(for: "SIGKILL"), 137)
        XCTAssertEqual(ExitSignals.code(for: "USR1"), 255, "numbers that differ between systems are not guessed")
    }
}

private struct KexHook: TransportHook {
    let kex: [String]

    func channelOptions() -> [CapstanSSH.SocketOption] { [] }
    func algorithms(pinnedHostKey: String?) -> AlgorithmPreferences {
        AlgorithmPreferences(hostKeys: SupportedAlgorithms.hostKeys, kex: kex, ciphers: SupportedAlgorithms.ciphers)
    }
    func terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void> {
        channel.close()
    }
}
