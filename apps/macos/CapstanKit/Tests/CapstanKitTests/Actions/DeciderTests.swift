import XCTest
@testable import CapstanKit

final class DeciderTests: XCTestCase {
    private var runner = FakeRunner()
    private var clock = TestClock()
    private var store = PendingStore(url: tempStoreURL())

    override func setUp() {
        runner = FakeRunner()
        clock = TestClock()
        store = PendingStore(url: tempStoreURL())
    }

    private func decider() -> Decider {
        Decider(runner: runner, pending: store, clock: clock)
    }

    private func send(_ script: FakeScript, verb: DecideVerb = .opDeny(note: nil)) async -> DecideOutcome {
        runner.enqueue(script)
        return await decider().send(makeRequest(verb: verb), config: makeConfig())
    }

    func testAppliedAndForwardedAndRepeat() async throws {
        let applied = await send(FakeScript(exit: 0, stdout: resultLine()))
        guard case .applied(let result) = applied else { return XCTFail("\(applied)") }
        XCTAssertFalse(result.isRepeat)
        XCTAssertEqual(result.eventSeq, 7)
        XCTAssertEqual(result.contentHash, testHash)

        let forwarded = await send(FakeScript(exit: 0, stdout: resultLine(outcome: "forwarded", replaces: "m1")))
        guard case .forwarded(let second) = forwarded else { return XCTFail("\(forwarded)") }
        XCTAssertEqual(second.replaces, "m1")
        XCTAssertEqual(second.messageId, "m2")

        let again = await send(FakeScript(exit: 0, stdout: resultLine(isRepeat: true)))
        guard case .applied(let repeated) = again else { return XCTFail("\(again)") }
        XCTAssertTrue(repeated.isRepeat)
    }

    func testTheSshCommandIsRunAndEventSeqMayBeMissing() async throws {
        let line = resultLine().replacingOccurrences(of: ",\"eventSeq\":7", with: "")
        _ = await send(FakeScript(exit: 0, stdout: line))
        XCTAssertEqual(runner.startCount, 1)
        let words = try runner.remoteWords(at: 0)
        XCTAssertEqual(Array(words.prefix(4)), ["cstan", "decide", "--project", "proj-1"])
        let again = await send(FakeScript(exit: 0, stdout: line))
        guard case .applied(let result) = again else { return XCTFail("\(again)") }
        XCTAssertNil(result.eventSeq)
    }

    func testEveryRefusalCodeMapsToAHint() async {
        let expected: [(String, DecideErrorCode, RefusalHint)] = [
            ("hash_mismatch", .hashMismatch, .refresh), ("not_waiting", .notWaiting, .gone),
            ("needs_terminal", .needsTerminal, .terminal), ("no_active_pm", .noActivePm, .noPm),
            ("limit_reached", .limitReached, .limit), ("nonce_reused", .nonceReused, .internal),
            ("invalid_request", .invalidRequest, .invalid), ("forbidden", .forbidden, .host),
            ("unauthorized", .unauthorized, .host), ("project_not_found", .projectNotFound, .host),
            ("no_operator_key", .noOperatorKey, .host), ("not_answering", .notAnswering, .host),
            ("something_new", .other("something_new"), .generic),
        ]
        XCTAssertEqual(DecideErrorCode.wireNames.count, 12)
        for (wire, code, hint) in expected {
            let outcome = await send(FakeScript(exit: 1, stdout: refusalLine(wire, message: "why")))
            guard case .refused(let refusal, let got) = outcome else { return XCTFail("\(wire): \(outcome)") }
            XCTAssertEqual(refusal.code, code, wire)
            XCTAssertEqual(refusal.message.raw, "why")
            XCTAssertEqual(got, hint, wire)
        }
        let kept = await store.all()
        XCTAssertTrue(kept.isEmpty)
    }

    func testExit255IsNotSentAndKept() async {
        let outcome = await send(FakeScript(exit: 255, stderr: "ssh: no route"))
        guard case .notSent(let reason, let pendingId) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(reason, .ssh(tail: hostText("ssh: no route")))
        let kept = await store.all()
        XCTAssertEqual(kept.map(\.id), [pendingId])
    }

    func testStartFailureIsNotSent() async {
        runner.failNextStart()
        let outcome = await decider().send(makeRequest(verb: .opApprove), config: makeConfig())
        guard case .notSent(.couldNotStart, _) = outcome else { return XCTFail("\(outcome)") }
        let kept = await store.all()
        XCTAssertEqual(kept.count, 1)
    }

    func testExit2IsMalformedAndRemoved() async {
        let outcome = await send(FakeScript(exit: 2, stderr: "usage"))
        guard case .malformed(_, _) = outcome else { return XCTFail("\(outcome)") }
        let kept = await store.all()
        XCTAssertTrue(kept.isEmpty)
    }

    func testNonJSONOutputIsMalformedAndKept() async {
        for script in [
            FakeScript(exit: 0, stdout: "Segmentation fault\n"), FakeScript(exit: 0, stdout: ""),
            FakeScript(exit: 0, stdout: resultLine() + resultLine()), FakeScript(exit: 3, stdout: resultLine()),
            FakeScript(exit: 143),
        ] {
            let outcome = await send(script)
            guard case .malformed(_, _) = outcome else { return XCTFail("\(outcome)") }
        }
        let kept = await store.all()
        XCTAssertEqual(kept.count, 5)
    }

    func testTimeoutTerminatesTheChildAndKeepsTheRequest() async throws {
        runner.enqueue(FakeScript())
        let decider = self.decider()
        let request = makeRequest(verb: .opApprove)
        let task = Task { await decider.send(request, config: makeConfig()) }
        await eventually("the timeout sleep") { self.clock.sleeperCount == 1 }
        clock.advance(by: 19)
        await settle()
        XCTAssertEqual(runner.child(at: 0).terminateCount, 0)
        clock.advance(by: 1)
        let outcome = await task.value
        guard case .notSent(.timeout, let pendingId) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(runner.child(at: 0).terminateCount, 1)
        let kept = await store.record(pendingId)
        XCTAssertEqual(kept?.request, request)
    }

    func testRetryReusesTheNonce() async throws {
        runner.enqueue(FakeScript(exit: 255))
        let decider = self.decider()
        let request = makeRequest(verb: .opDeny(note: "n"))
        let first = await decider.send(request, config: makeConfig())
        guard case .notSent(_, let id) = first else { return XCTFail("\(first)") }
        runner.enqueue(FakeScript(exit: 0, stdout: resultLine(isRepeat: true)))
        let second = await decider.retry(id, config: makeConfig())
        guard case .applied(let result) = second else { return XCTFail("\(second)") }
        XCTAssertTrue(result.isRepeat)
        XCTAssertEqual(try runner.remoteWords(at: 0), try runner.remoteWords(at: 1))
        XCTAssertEqual(flagValue("--nonce", in: try runner.remoteWords(at: 1)), testNonce)
        let kept = await store.all()
        XCTAssertTrue(kept.isEmpty)
    }

    func testRetryOfUnknownId() async {
        let outcome = await decider().retry(UUID(), config: makeConfig())
        guard case .malformed(_, _) = outcome else { return XCTFail("\(outcome)") }
    }

    func testNonJSONExitZeroKeepsTheNonceForRetry() async throws {
        let first = await send(FakeScript(exit: 0, stdout: "oops\n"))
        guard case .malformed(_, _) = first else { return XCTFail("\(first)") }
        let kept = await store.all()
        let id = try XCTUnwrap(kept.first?.id)
        runner.enqueue(FakeScript(exit: 0, stdout: resultLine(isRepeat: true)))
        let second = await decider().retry(id, config: makeConfig())
        guard case .applied = second else { return XCTFail("\(second)") }
        XCTAssertEqual(flagValue("--nonce", in: try runner.remoteWords(at: 1)), testNonce)
    }
}
