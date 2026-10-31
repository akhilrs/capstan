import CapstanKit
import Foundation
import NIOCore
import XCTest
@testable import CapstanSSH

/// One connection per host, channels on it, idle close, suspend, and starts that open nothing.
final class RunnerConnectionTests: XCTestCase {
    private var fixture: RunnerFixture!

    override func setUpWithError() throws {
        fixture = try RunnerFixture()
    }

    override func tearDown() {
        fixture.stop()
        fixture = nil
    }

    private func waitForExecs(_ count: Int) async {
        await eventually(timeout: 5, "\(count) exec requests") { self.fixture.server.log.execs.count >= count }
    }

    func testTwoStartsShareOneConnection() async throws {
        let slow = try fixture.start("sleep 1; echo slow")
        let quick = try fixture.start("echo quick")
        let quickCode = await exitCode(quick)
        let slowCode = await exitCode(slow)
        XCTAssertEqual(quickCode, 0)
        XCTAssertEqual(slowCode, 0)
        XCTAssertEqual(fixture.server.log.connections, 1)
    }

    func testTerminateClosesOnlyItsChannel() async throws {
        let long = try fixture.start("sleep 30")
        let other = try fixture.start("sleep 1; echo still here")
        await waitForExecs(2)
        long.terminate()
        let longCode = await exitCode(long)
        XCTAssertEqual(longCode, 255)
        let result = await collect(other)
        XCTAssertEqual(result.lines, ["still here"])
        let otherCode = await exitCode(other)
        XCTAssertEqual(otherCode, 0)
        XCTAssertEqual(fixture.server.log.connections, 1)
    }

    func testIdleCloseAfterSixtySeconds() async throws {
        let process = try fixture.start("echo one")
        _ = await exitCode(process)
        await eventually(timeout: 5, "the idle timer") { self.fixture.clock.sleeperCount == 1 }

        fixture.clock.advance(by: 59)
        await settle()
        XCTAssertEqual(fixture.server.log.disconnects, 0, "not idle long enough yet")

        fixture.clock.advance(by: 1)
        await eventually(timeout: 5, "the idle close") { self.fixture.server.log.disconnects == 1 }

        let next = try fixture.start("echo two")
        let code = await exitCode(next)
        XCTAssertEqual(code, 0)
        XCTAssertEqual(fixture.server.log.connections, 2, "the next start reconnects")
    }

    func testANewChannelStopsTheIdleClose() async throws {
        let first = try fixture.start("echo one")
        _ = await exitCode(first)
        await eventually(timeout: 5, "the idle timer") { self.fixture.clock.sleeperCount == 1 }
        let second = try fixture.start("echo two")
        _ = await exitCode(second)
        fixture.clock.advance(by: 30)
        await settle()
        XCTAssertEqual(fixture.server.log.disconnects, 0)
        XCTAssertEqual(fixture.server.log.connections, 1)
    }

    func testSuspendEndsOpenChannelsAndTheNextStartReconnects() async throws {
        let long = try fixture.start("sleep 30")
        await waitForExecs(1)
        fixture.runner.suspend()
        let code = await exitCode(long)
        XCTAssertEqual(code, 255)
        XCTAssertTrue(long.stderrTail().contains("suspended"), long.stderrTail())

        let next = try fixture.start("echo back")
        let nextCode = await exitCode(next)
        XCTAssertEqual(nextCode, 0)
        XCTAssertEqual(fixture.server.log.connections, 2)
    }

    func testStartsThatOpenNothing() async throws {
        let ssh = URL(fileURLWithPath: RemoteCommand.sshPath)
        let arguments = RemoteCommand.sshArguments(fixture.config, remote: "echo x")
        XCTAssertThrowsError(try fixture.runner.start(URL(fileURLWithPath: "/bin/sh"), arguments)) {
            XCTAssertEqual($0 as? SSHRunnerStartError, .notSSH)
        }
        XCTAssertThrowsError(try fixture.runner.start(ssh, ["-x", "--", fixture.host.alias, "echo x"])) {
            XCTAssertEqual($0 as? SSHRunnerStartError, .unparsableArguments)
        }
        var withKey = fixture.config
        withKey.identityFile = "/keys/id"
        XCTAssertThrowsError(try fixture.runner.start(ssh, RemoteCommand.sshArguments(withKey, remote: "echo x"))) {
            XCTAssertEqual($0 as? SSHRunnerStartError, .identityFile)
        }
        var unknown = fixture.config
        unknown.destination = "capstan-000000000000"
        XCTAssertThrowsError(try fixture.runner.start(ssh, RemoteCommand.sshArguments(unknown, remote: "echo x"))) {
            XCTAssertEqual($0 as? SSHRunnerStartError, .unknownDestination)
        }
        try await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertEqual(fixture.server.log.connections, 0)
    }

    func testSocketOptionsFromTheHookAreApplied() async throws {
        let options = [SocketOption(level: 0xFFFF, name: 0x0008, value: 1)]
        let custom = try RunnerFixture(hook: OptionsHook(options: options))
        defer { custom.stop() }
        let seen = SeenOptions()
        custom.runner.observeSocketOptions { seen.set($0) }
        let process = try custom.start("echo ok")
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
        XCTAssertEqual(seen.value, options)
    }
}

private final class SeenOptions: @unchecked Sendable {
    private let lock = NSLock()
    private var options: [SocketOption]?

    func set(_ value: [SocketOption]) { lock.withLock { options = value } }
    var value: [SocketOption]? { lock.withLock { options } }
}

/// SO_KEEPALIVE (SOL_SOCKET 0xffff, SO_KEEPALIVE 0x0008 on Darwin), the library's algorithms, close-only terminate.
private struct OptionsHook: TransportHook {
    let options: [SocketOption]

    func channelOptions() -> [SocketOption] { options }
    func algorithms(pinnedHostKey: String?) -> AlgorithmPreferences { SupportedAlgorithms.library }
    func terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void> {
        channel.close()
    }
}
