import XCTest
@testable import CapstanKit

final class ProbeTests: XCTestCase {
    func testBothVerbsAmongJunk() async throws {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 0, stdout: "Welcome\ncap events\nnoise cap decide\ncap decide\n"))
        let caps = await probeCapabilities(makeConfig(), runner)
        XCTAssertEqual(caps, HostCapabilities(reachable: true, events: true, decide: true, error: nil))
        let words = try runner.remoteWords(at: 0)
        XCTAssertEqual(words.count, 3)
        XCTAssertEqual(Array(words.prefix(2)), ["sh", "-c"])
        XCTAssertTrue(words[2].contains("'cstan' help events >/dev/null 2>&1 && echo cap events"), words[2])
        XCTAssertTrue(words[2].contains("'cstan' help decide >/dev/null 2>&1 && echo cap decide"), words[2])
    }

    func testOnlyCapLinesCount() async {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 0, stdout: "events\ncap events extra\nxcap decide\n"))
        let caps = await probeCapabilities(makeConfig(), runner)
        XCTAssertTrue(caps.reachable)
        XCTAssertFalse(caps.events)
        XCTAssertFalse(caps.decide)
    }

    func testSshFailureIsNotReachable() async {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 255, stderr: "no route"))
        let caps = await probeCapabilities(makeConfig(), runner)
        XCTAssertEqual(caps, HostCapabilities(
            reachable: false, events: false, decide: false, error: .ssh(tail: hostText("no route"))))
    }

    func testCheckHostReportsEachStep() async {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 0))
        runner.enqueue(FakeScript(exit: 0, stdout: "cap events\ncap decide\n"))
        runner.enqueue(FakeScript(chunks: [
            Wire.hello, Wire.project("p1", state: "ready"), Wire.project("p2", state: "no_key"), Wire.snapshotEnd,
        ]))
        var checks: [HostCheck] = []
        for await check in checkHost(makeConfig(), runner, clock: TestClock()) { checks.append(check) }
        XCTAssertEqual(checks.map(\.step), [.connect, .capstan, .projects, .firstSnapshot])
        XCTAssertEqual(checks.map(\.status), [.passed, .passed, .warning, .passed])
        XCTAssertEqual(checks[2].text, "1 of 2 projects ready")
        XCTAssertEqual(runner.child(at: 2).terminateCount, 1)
    }

    func testCheckHostStopsAtMissingCstan() async {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 0))
        runner.enqueue(FakeScript(exit: 0, stdout: ""))
        var checks: [HostCheck] = []
        for await check in checkHost(makeConfig(), runner, clock: TestClock()) { checks.append(check) }
        XCTAssertEqual(checks.map(\.step), [.connect, .capstan])
        XCTAssertEqual(checks.map(\.status), [.passed, .failed])
        XCTAssertEqual(runner.startCount, 2)
    }

    func testCheckHostFailedConnect() async {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 255, stderr: "Host key verification failed."))
        var checks: [HostCheck] = []
        for await check in checkHost(makeConfig(), runner, clock: TestClock()) { checks.append(check) }
        XCTAssertEqual(checks.map(\.status), [.failed])
        XCTAssertTrue(checks[0].text.contains("Host key verification failed."))
    }
}
