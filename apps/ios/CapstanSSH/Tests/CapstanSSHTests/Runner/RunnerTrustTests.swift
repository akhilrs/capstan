import CapstanKit
import CapstanPhoneCore
import Foundation
import XCTest
@testable import CapstanSSH

/// Host keys, auth and events.
final class RunnerTrustTests: XCTestCase {
    private var fixtures: [RunnerFixture] = []

    private func make(
        trusted: Bool = true, serverAccepts: Bool = true, pinsFile: Data? = nil
    ) throws -> RunnerFixture {
        let fixture = try RunnerFixture(trusted: trusted, serverAccepts: serverAccepts, pinsFile: pinsFile)
        fixtures.append(fixture)
        return fixture
    }

    override func tearDown() {
        for fixture in fixtures { fixture.stop() }
        fixtures = []
    }

    func testUnknownHostKeyEmitsAnEventAndRunsNothing() async throws {
        let fixture = try make(trusted: false)
        let events = EventRecorder(fixture.runner.events)
        let process = try fixture.start("echo ran")
        let code = await exitCode(process)
        XCTAssertEqual(code, 255)
        XCTAssertTrue(process.stderrTail().contains("not trusted"), process.stderrTail())
        XCTAssertEqual(fixture.server.log.execs, [], "the command did not run")
        let seen = await events.wait(for: 1)
        XCTAssertEqual(seen.first, .hostKeyUnknown(hostId: fixture.host.id, pin: fixture.server.hostPin))
    }

    func testAfterTrustTheNextStartRuns() async throws {
        let fixture = try make(trusted: false)
        let first = try fixture.start("echo ran")
        let firstCode = await exitCode(first)
        XCTAssertEqual(firstCode, 255)

        XCTAssertTrue(fixture.pins.trust(hostId: fixture.host.id, pin: fixture.server.hostPin))
        XCTAssertNil(fixture.runner.latestEvent(hostId: fixture.host.id), "trust() clears the event")
        let second = try fixture.start("echo ran")
        let result = await collect(second)
        XCTAssertEqual(result.lines, ["ran"])
        let code = await exitCode(second)
        XCTAssertEqual(code, 0)
    }

    func testChangedHostKeyIsBlockedEveryTime() async throws {
        let fixture = try make(trusted: false)
        fixture.pins.trust(hostId: fixture.host.id, pin: HostKeyPin(algorithm: "ssh-ed25519", blob: Data(repeating: 7, count: 51)))
        let events = EventRecorder(fixture.runner.events)
        for _ in 0..<2 {
            let process = try fixture.start("echo ran")
            let code = await exitCode(process)
            XCTAssertEqual(code, 255)
            XCTAssertTrue(process.stderrTail().contains("host key changed"), process.stderrTail())
        }
        XCTAssertEqual(fixture.server.log.execs, [])
        let seen = await events.wait(for: 2)
        XCTAssertEqual(seen, [.hostKeyChanged(hostId: fixture.host.id), .hostKeyChanged(hostId: fixture.host.id)])
        XCTAssertEqual(fixture.runner.latestEvent(hostId: fixture.host.id), .hostKeyChanged(hostId: fixture.host.id))
    }

    func testUnreadablePinsConnectToNothing() async throws {
        let fixture = try make(pinsFile: Data("{broken".utf8))
        let events = EventRecorder(fixture.runner.events)
        let process = try fixture.start("echo ran")
        let code = await exitCode(process)
        XCTAssertEqual(code, 255)
        let seen = await events.wait(for: 1)
        XCTAssertEqual(seen, [.pinsUnreadable])
        XCTAssertEqual(fixture.runner.latestEvent(hostId: fixture.host.id), .pinsUnreadable)
        try await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertEqual(fixture.server.log.connections, 0)
    }

    func testRefusedKeyGivesAuthRefusedAndNoPassword() async throws {
        let fixture = try make(serverAccepts: false)
        let events = EventRecorder(fixture.runner.events)
        let process = try fixture.start("echo ran")
        let code = await exitCode(process)
        XCTAssertEqual(code, 255)
        XCTAssertTrue(process.stderrTail().contains("refused this iPhone's key"), process.stderrTail())
        let seen = await events.wait(for: 1)
        XCTAssertEqual(seen, [.authRefused(hostId: fixture.host.id)])
        XCTAssertFalse(fixture.server.log.authMethods.isEmpty)
        XCTAssertFalse(fixture.server.log.authMethods.contains("password"), "\(fixture.server.log.authMethods)")
        XCTAssertEqual(fixture.server.log.execs, [])
    }

    func testAnEventSentBeforeTheConsumerSubscribesIsStillRead() async throws {
        let fixture = try make(trusted: false)
        let process = try fixture.start("echo ran")
        _ = await exitCode(process)
        XCTAssertEqual(
            fixture.runner.latestEvent(hostId: fixture.host.id),
            .hostKeyUnknown(hostId: fixture.host.id, pin: fixture.server.hostPin))
        // Subscribed only now.
        let events = EventRecorder(fixture.runner.events)
        let seen = await events.wait(for: 1)
        XCTAssertEqual(seen, [.hostKeyUnknown(hostId: fixture.host.id, pin: fixture.server.hostPin)])
    }

    func testASuccessfulStartClearsTheLatestEvent() async throws {
        let fixture = try make(trusted: false)
        let refused = try fixture.start("echo ran")
        _ = await exitCode(refused)
        fixture.pins.trust(hostId: fixture.host.id, pin: fixture.server.hostPin)
        let process = try fixture.start("echo ran")
        _ = await exitCode(process)
        XCTAssertNil(fixture.runner.latestEvent(hostId: fixture.host.id))
    }
}
