import Foundation
import XCTest
@testable import CapstanKit

final class RecentCallTests: XCTestCase {
    private let fixtures = RepoPaths.repoRoot.appendingPathComponent("rust/crates/cstan/tests/fixtures/recent")

    private func fixture(_ name: String) throws -> Data {
        try Data(contentsOf: fixtures.appendingPathComponent(name))
    }

    private func call() -> RecentCall {
        RecentCall(projectId: "proj-a", epoch: RecentSamples.epoch, head: 26)
    }

    private func result(_ text: String, stderr: String = "", exit: Int32) -> RecentResult {
        RecentCall.result(stdout: Data(text.utf8), stderr: stderr, exitCode: exit)
    }

    func testTheArguments() throws {
        let words = try call().remoteArguments()
        let expected = ["recent", "--project", "proj-a", "--epoch", RecentSamples.epoch, "--head", "26", "--json"]
        XCTAssertEqual(words, expected)
        let ssh = try call().sshArguments(makeConfig())
        let remote = try XCTUnwrap(ssh.last)
        XCTAssertTrue(remote.contains("'recent' '--project' 'proj-a'"))
    }

    func testAValueThatWouldNotBeAcceptedIsRefusedBeforeAnythingIsSent() {
        XCTAssertThrowsError(try RecentCall(projectId: "-x", epoch: RecentSamples.epoch, head: 1).remoteArguments())
        XCTAssertThrowsError(try RecentCall(projectId: "a b", epoch: RecentSamples.epoch, head: 1).remoteArguments())
        XCTAssertThrowsError(try RecentCall(projectId: "p", epoch: "XYZ", head: 1).remoteArguments())
        XCTAssertThrowsError(try RecentCall(projectId: "p", epoch: RecentSamples.epoch, head: -1).remoteArguments())
    }

    func testTheUsageTextOfACstanWithoutTheCommandIsSessionOnly() {
        let usage = "cstan: unknown command: recent\nrun `cstan help` for the list of commands\n"
        XCTAssertEqual(result("", stderr: usage, exit: 2), .sessionOnly)
        XCTAssertEqual(result(usage, exit: 2), .sessionOnly)
    }

    func testOutputThatIsNotJsonIsAnErrorCarryingItsText() {
        let outcome = result("", stderr: "bash: cstan: command not found", exit: 127)
        XCTAssertEqual(outcome, .error(code: "unexpected_output", message: "bash: cstan: command not found"))
    }

    func testAnotherUsageErrorWithExitTwoIsNotSessionOnly() {
        let outcome = result("", stderr: "cstan recent: bad flag --x", exit: 2)
        XCTAssertEqual(outcome, .error(code: "unexpected_output", message: "cstan recent: bad flag --x"))
    }

    func testTheFixturesOfTheCommandMapToSessionOnly() throws {
        XCTAssertEqual(RecentCall.result(stdout: try fixture("unknown_command.json"), stderr: "", exitCode: 1), .sessionOnly)
        XCTAssertEqual(RecentCall.result(stdout: try fixture("old_usage.json"), stderr: "", exitCode: 1), .sessionOnly)
    }

    func testARejectedAnswerWithTheRateLimitPrefixIsRateLimited() {
        let limited = "{\"v\":1,\"error\":{\"code\":\"rejected\",\"message\":\"rate_limited: 30 reads a minute\"}}\n"
        XCTAssertEqual(result(limited, exit: 1), .rateLimited)
    }

    func testAnotherRejectedAnswerIsAnError() {
        let busy = "{\"v\":1,\"error\":{\"code\":\"rejected\",\"message\":\"busy\"}}\n"
        XCTAssertEqual(result(busy, exit: 1), .error(code: "rejected", message: "busy"))
        let forbidden = "{\"v\":1,\"error\":{\"code\":\"forbidden\",\"message\":\"no\"}}\n"
        XCTAssertEqual(result(forbidden, exit: 1), .error(code: "forbidden", message: "no"))
    }

    func testSshItselfFailingIsAnError() {
        let outcome = result("", stderr: "ssh: connect to host x port 22: Connection refused", exit: 255)
        guard case .error(let code, _) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(code, "ssh")
    }

    func testAPageIsReadNewestFirstAndTruncatedIsKept() throws {
        let text = RecentSamples.page(events: [RecentSamples.event(seq: 7), RecentSamples.event(seq: 9)], truncated: true)
        let outcome = result(text + "\n", exit: 0)
        guard case .page(let page) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(page.events.map { $0.seq }, [9, 7])
        XCTAssertTrue(page.truncated)
        XCTAssertEqual(page.epoch, RecentSamples.epoch)
        XCTAssertEqual(page.head, 9)
    }

    func testAResyncAnswerCarriesTheLedgersOwnPosition() {
        let text = "{\"projectId\":\"pfixture\",\"epoch\":\"\(RecentSamples.epoch)\",\"head\":4}\n"
        XCTAssertEqual(result(text, exit: 1), .resync(epoch: RecentSamples.epoch, head: 4))
    }

    func testRunningTheCallParsesTheChildsAnswer() async throws {
        let runner = FakeRunner()
        runner.enqueue(FakeScript(exit: 0, stdout: RecentSamples.page(events: []) + "\n"))
        let outcome = await call().run(runner: runner, config: makeConfig(), clock: TestClock())
        guard case .page(let page) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertTrue(page.events.isEmpty)
        let words = try runner.remoteWords(at: 0)
        XCTAssertEqual(Array(words.prefix(2)), ["cstan", "recent"])
    }

    func testACallThatCannotStartIsAnErrorResult() async {
        let runner = FakeRunner()
        runner.failNextStart()
        let outcome = await call().run(runner: runner, config: makeConfig(), clock: TestClock())
        guard case .error(let code, _) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(code, "not_started")
    }

    func testACallThatDoesNotAnswerWithinTenSecondsIsStopped() async throws {
        XCTAssertEqual(RecentCall.timeoutSeconds, 10)
        let runner = FakeRunner()
        let clock = TestClock()
        runner.enqueue(FakeScript())
        let sent = call()
        let config = makeConfig()
        let pending = Task { await sent.run(runner: runner, config: config, clock: clock) }
        await eventually("the timeout sleeper") { clock.sleeperCount == 1 }
        clock.advance(by: 11)
        let outcome = await pending.value
        guard case .error(let code, _) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(code, "timeout")
        XCTAssertEqual(runner.child(at: 0).terminateCount, 1)
    }
}
