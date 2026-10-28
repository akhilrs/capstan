import XCTest
@testable import CapstanKit

final class HostSessionTests: XCTestCase {
    private var runner = FakeRunner()
    private var clock = TestClock()
    private var consumer = RecordingConsumer()

    override func setUp() {
        runner = FakeRunner()
        clock = TestClock()
        consumer = RecordingConsumer()
    }

    private func makeSession(config: HostConfig = makeConfig(), consumer: RecordingConsumer? = nil) -> HostSession {
        HostSession(
            config: config, runner: runner, consumer: consumer ?? self.consumer, clock: clock, jitter: { 1.0 })
    }

    /// Starts the session and waits until the first child exists and the session has noted its start time.
    private func startAndWait(_ session: HostSession) async {
        await session.start()
        await eventually("first start") { self.runner.startCount >= 1 }
        await settle()
    }

    private func waitForState(_ message: String, _ match: @escaping (ConnectionState) -> Bool) async {
        await eventually(message) { await self.consumer.lastState().map(match) ?? false }
        // Let the session reach its next sleep before a test moves the clock.
        await settle()
    }

    private func isReconnecting(_ state: ConnectionState, attempt: Int) -> Bool {
        if case .reconnecting(let number, _, _) = state { return number == attempt }
        return false
    }

    private func reconnectError(_ state: ConnectionState?) -> SessionError? {
        if case .reconnecting(_, _, let error)? = state { return error }
        return nil
    }

    // MARK: Lines and batches

    func testLinesSplitAcrossChunksArriveWhole() async throws {
        let cursor = makeCursor(["p1"])
        let session = makeSession()
        runner.enqueue(FakeScript(chunks: [
            String(Wire.hello.prefix(10)), String(Wire.hello.dropFirst(10).prefix(15)),
            String(Wire.hello.dropFirst(25)) + String(Wire.cursor(cursor).prefix(20)),
            String(Wire.cursor(cursor).dropFirst(20)),
        ]))
        await startAndWait(session)
        await eventually("one batch") { await self.consumer.batches.count == 1 }
        let batch = await consumer.batches[0]
        XCTAssertEqual(batch.count, 2)
        XCTAssertEqual(batch[0], .hello(Hello(version: "0.1", host: "box", projects: 2)))
        XCTAssertEqual(batch[1], .cursor(cursor))
        let state = await consumer.lastState()
        guard case .live? = state else { return XCTFail("expected live, got \(String(describing: state))") }
    }

    func testBatchesEndAtCursorLinesAndApplyIsAwaited() async throws {
        let first = makeCursor(["p1"], sequence: 1)
        let second = makeCursor(["p1"], sequence: 2)
        let gate = Latch<Void>()
        await consumer.hold(gate)
        runner.enqueue(FakeScript(chunks: [
            Wire.hello + Wire.project("p1", state: "ready") + Wire.cursor(first),
            Wire.heartbeat + Wire.hello + Wire.cursor(second),
        ]))
        await startAndWait(makeSession())
        await settle()
        let before = await consumer.batches
        XCTAssertTrue(before.isEmpty)
        gate.set(())
        await eventually("two batches") { await self.consumer.batches.count == 2 }
        let batches = await consumer.batches
        XCTAssertEqual(batches[0].count, 3)
        XCTAssertEqual(batches[0].last, .cursor(first))
        // The heartbeat resets the watchdog but is not delivered.
        XCTAssertEqual(batches[1].count, 2)
        XCTAssertEqual(batches[1].last, .cursor(second))
    }

    func testEndWithoutFinalCursorReconnectsWithLastCursor() async throws {
        let cursor = makeCursor(["p1"])
        runner.enqueue(FakeScript(chunks: [Wire.hello + Wire.cursor(cursor) + Wire.hello], exit: 0))
        let session = makeSession()
        await startAndWait(session)
        await waitForState("reconnecting") { self.isReconnecting($0, attempt: 1) }
        let batches = await consumer.batches
        XCTAssertEqual(batches.count, 2)
        XCTAssertEqual(batches[1], [.hello(Hello(version: "0.1", host: "box", projects: 2))])
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .interrupted)
        clock.advance(by: 1)
        await eventually("second start") { self.runner.startCount == 2 }
        let words = try runner.remoteWords(at: 1)
        XCTAssertEqual(flagValue("--cursor", in: words), cursor.text)
        XCTAssertNil(flagValue("--cursor", in: try runner.remoteWords(at: 0)))
    }

    func testResumeCursorFromConsumerAndRoots() async throws {
        let cursor = makeCursor(["p1"])
        let session = makeSession(config: makeConfig(pollMs: 3000, roots: ["/work/x"]), consumer: RecordingConsumer(cursor: cursor))
        await session.start()
        await eventually("start") { self.runner.startCount == 1 }
        XCTAssertEqual(try runner.remoteWords(at: 0), [
            "cstan", "events", "--poll-ms", "3000", "--cursor", cursor.text, "--root", "/work/x",
        ])
    }

    // MARK: Exit codes and backoff

    func testSshFailureRetriesAfterOneThenTwoSeconds() async throws {
        runner.enqueue(FakeScript(exit: 255, stderr: "ssh: could not resolve"))
        runner.enqueue(FakeScript(exit: 255, stderr: "ssh: could not resolve"))
        await startAndWait(makeSession())
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .ssh(tail: hostText("ssh: could not resolve")))
        clock.advance(by: 0.9)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
        clock.advance(by: 0.1)
        await eventually("second start") { self.runner.startCount == 2 }
        await waitForState("attempt 2") { self.isReconnecting($0, attempt: 2) }
        clock.advance(by: 1.9)
        await settle()
        XCTAssertEqual(runner.startCount, 2)
        clock.advance(by: 0.1)
        await eventually("third start") { self.runner.startCount == 3 }
    }

    func testExitTwoStopsWithoutRetry() async throws {
        runner.enqueue(FakeScript(exit: 2, stderr: "usage: cstan events"))
        await startAndWait(makeSession())
        await waitForState("stopped") { $0 == .stopped(.hostRejected(code: nil, tail: hostText("usage: cstan events"))) }
        clock.advance(by: 100)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
    }

    func testExit127StopsWithCstanMissing() async throws {
        runner.enqueue(FakeScript(exit: 127, stderr: "cstan: not found"))
        await startAndWait(makeSession())
        await waitForState("stopped") { $0 == .stopped(.cstanMissing(tail: hostText("cstan: not found"))) }
        clock.advance(by: 100)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
    }

    func testKilledChildReconnectsWithExitCode() async throws {
        runner.enqueue(FakeScript(exit: 137, stderr: "Killed"))
        await startAndWait(makeSession())
        await waitForState("reconnecting") { self.isReconnecting($0, attempt: 1) }
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .exit(137, tail: hostText("Killed")))
    }

    func testInternalErrorLineReconnectsAndFifthInARowStops() async throws {
        for _ in 0..<5 { runner.enqueue(FakeScript(chunks: [Wire.error("internal", "db busy")], exit: 1)) }
        await startAndWait(makeSession())
        for (index, delay) in [1.0, 2.0, 4.0, 8.0].enumerated() {
            await waitForState("attempt \(index + 1)") { self.isReconnecting($0, attempt: index + 1) }
            let last = await consumer.lastState()
            XCTAssertEqual(reconnectError(last), .stream(code: "internal", message: hostText("db busy")))
            clock.advance(by: delay)
            await eventually("start \(index + 2)") { self.runner.startCount == index + 2 }
        }
        await waitForState("stopped") { $0 == .stopped(.repeatedFailure) }
        clock.advance(by: 100)
        await settle()
        XCTAssertEqual(runner.startCount, 5)
    }

    func testErrorLineThenExitZeroKeepsTheErrorAndCounts() async throws {
        for _ in 0..<5 { runner.enqueue(FakeScript(chunks: [Wire.error("internal", "db busy")], exit: 0)) }
        await startAndWait(makeSession())
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .stream(code: "internal", message: hostText("db busy")))
        for (index, delay) in [1.0, 2.0, 4.0, 8.0].enumerated() {
            if index > 0 { await waitForState("attempt \(index + 1)") { self.isReconnecting($0, attempt: index + 1) } }
            clock.advance(by: delay)
            await eventually("start \(index + 2)") { self.runner.startCount == index + 2 }
        }
        await waitForState("stopped") { $0 == .stopped(.repeatedFailure) }
    }

    func testRejectingErrorLinesStopWithoutRetry() async throws {
        for code in ["unauthorized", "forbidden", "unknown_command", "invalid_request"] {
            setUp()
            runner.enqueue(FakeScript(chunks: [Wire.error(code)], exit: 1, stderr: "denied"))
            await startAndWait(makeSession())
            await waitForState("stopped \(code)") { $0 == .stopped(.hostRejected(code: code, tail: hostText("denied"))) }
            clock.advance(by: 100)
            await settle()
            XCTAssertEqual(runner.startCount, 1, code)
        }
    }

    func testBadCursorDropsItAndReconnectsWithout() async throws {
        let cursor = makeCursor(["p1"])
        let consumer = RecordingConsumer(cursor: cursor)
        runner.enqueue(FakeScript(chunks: [Wire.error("bad_cursor")], exit: 1))
        let session = makeSession(consumer: consumer)
        await session.start()
        await eventually("second start") { self.runner.startCount == 2 }
        XCTAssertEqual(flagValue("--cursor", in: try runner.remoteWords(at: 0)), cursor.text)
        XCTAssertNil(flagValue("--cursor", in: try runner.remoteWords(at: 1)))
        let drops = await consumer.dropCount
        XCTAssertEqual(drops, 1)
    }

    func testBadCursorWithoutACursorBacksOff() async throws {
        runner.enqueue(FakeScript(chunks: [Wire.error("bad_cursor", "no")], exit: 1))
        await startAndWait(makeSession())
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .stream(code: "bad_cursor", message: hostText("no")))
        let drops = await consumer.dropCount
        XCTAssertEqual(drops, 0)
        XCTAssertEqual(runner.startCount, 1)
        clock.advance(by: 1)
        await eventually("second start") { self.runner.startCount == 2 }
    }

    func testUnsupportedVersionTerminatesAndStops() async throws {
        runner.enqueue(FakeScript(chunks: ["{\"v\":2,\"kind\":\"hello\"}\n"]))
        await startAndWait(makeSession())
        await waitForState("stopped") { $0 == .stopped(.unsupportedVersion(2)) }
        XCTAssertGreaterThanOrEqual(runner.child(at: 0).terminateCount, 1)
        clock.advance(by: 100)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
    }

    func testTooLongLineReconnects() async throws {
        runner.enqueue(FakeScript(chunks: [String(repeating: "a", count: 65536) + "\n"]))
        await startAndWait(makeSession())
        await waitForState("reconnecting") { self.isReconnecting($0, attempt: 1) }
        XCTAssertGreaterThanOrEqual(runner.child(at: 0).terminateCount, 1)
        clock.advance(by: 1)
        await eventually("second start") { self.runner.startCount == 2 }
    }

    func testMalformedLineReconnects() async throws {
        runner.enqueue(FakeScript(chunks: ["not json\n"]))
        await startAndWait(makeSession())
        await waitForState("reconnecting") { self.isReconnecting($0, attempt: 1) }
        let last = await consumer.lastState()
        XCTAssertEqual(reconnectError(last), .interrupted)
    }

    func testLiveForAMinuteResetsTheBackoff() async throws {
        runner.enqueue(FakeScript(exit: 255))
        let session = makeSession()
        await startAndWait(session)
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        clock.advance(by: 1)
        await eventually("second start") { self.runner.startCount == 2 }
        await settle()
        runner.child(at: 1).send(Wire.heartbeat)
        await waitForState("live") { if case .live = $0 { return true } else { return false } }
        clock.advance(by: 20)
        runner.child(at: 1).send(Wire.heartbeat)
        await settle()
        clock.advance(by: 20)
        runner.child(at: 1).send(Wire.heartbeat)
        await settle()
        clock.advance(by: 21)
        runner.child(at: 1).end(exit: 255)
        await waitForState("attempt 1 again") { self.isReconnecting($0, attempt: 1) }
    }

    // MARK: Heartbeat

    func testHeartbeatTimeoutIsThreeTimesTheLongerInterval() async throws {
        await startAndWait(makeSession())
        clock.advance(by: 44)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
        let early = await consumer.states
        XCTAssertFalse(early.contains { if case .noHeartbeat = $0 { return true } else { return false } })
        clock.advance(by: 1)
        await eventually("restart") { self.runner.startCount == 2 }
        let states = await consumer.states
        XCTAssertTrue(states.contains { if case .noHeartbeat = $0 { return true } else { return false } })
        XCTAssertGreaterThanOrEqual(runner.child(at: 0).terminateCount, 1)
    }

    func testRestartMidReadKeepsTheNewWatchdog() async throws {
        let session = makeSession()
        await startAndWait(session)
        runner.child(at: 0).send(Wire.hello)
        await settle()
        await session.restart()
        await eventually("second start") { self.runner.startCount == 2 }
        await settle()
        clock.advance(by: 44)
        await settle()
        XCTAssertEqual(runner.startCount, 2)
        clock.advance(by: 1)
        await eventually("third start") { self.runner.startCount == 3 }
        let states = await consumer.states
        XCTAssertTrue(states.contains { if case .noHeartbeat = $0 { return true } else { return false } })
    }

    func testSlowPollStretchesTheTimeout() async throws {
        await startAndWait(makeSession(config: makeConfig(pollMs: 20_000)))
        clock.advance(by: 59)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
        clock.advance(by: 1)
        await eventually("restart") { self.runner.startCount == 2 }
    }

    func testHeartbeatAndUnknownKindResetTheWatchdog() async throws {
        for line in [Wire.heartbeat, Wire.future] {
            setUp()
            await startAndWait(makeSession())
            clock.advance(by: 40)
            runner.child(at: 0).send(line)
            await settle()
            clock.advance(by: 40)
            await settle()
            XCTAssertEqual(runner.startCount, 1)
            clock.advance(by: 5)
            await eventually("restart") { self.runner.startCount == 2 }
        }
    }

    // MARK: Refresh

    func testTwoRefreshesInOneSecondGiveOneRestartWithoutThoseProjects() async throws {
        let cursor = makeCursor(["p1", "p2", "p3"])
        let session = makeSession(consumer: RecordingConsumer(cursor: cursor))
        await startAndWait(session)
        await session.refresh(projectIds: ["p1"])
        await settle()
        clock.advance(by: 0.5)
        await session.refresh(projectIds: ["p2"])
        await settle()
        clock.advance(by: 0.5)
        await eventually("restart") { self.runner.startCount == 2 }
        await settle()
        XCTAssertEqual(runner.startCount, 2)
        XCTAssertEqual(runner.child(at: 0).terminateCount, 1)
        let text = try XCTUnwrap(flagValue("--cursor", in: try runner.remoteWords(at: 1)))
        let payload = try XCTUnwrap(Cursor(text: text)).decoded()
        XCTAssertEqual(Set(payload.p.keys), ["p3"])
    }

    func testRefreshOfEveryProjectRestartsWithoutACursor() async throws {
        let cursor = makeCursor(["p1", "p2"])
        let session = makeSession(consumer: RecordingConsumer(cursor: cursor))
        await startAndWait(session)
        await session.refresh(projectIds: ["p1", "p2"])
        await settle()
        clock.advance(by: 1)
        await eventually("restart") { self.runner.startCount == 2 }
        XCTAssertNil(flagValue("--cursor", in: try runner.remoteWords(at: 1)))
    }

    func testRefreshWithAnUnreadableCursorRestartsWithoutACursor() async throws {
        let bad = try XCTUnwrap(Cursor(text: "c1.AAAA"))
        let session = makeSession(consumer: RecordingConsumer(cursor: bad))
        await startAndWait(session)
        await session.refresh(projectIds: ["p1"])
        await settle()
        clock.advance(by: 1)
        await eventually("restart") { self.runner.startCount == 2 }
        XCTAssertNil(flagValue("--cursor", in: try runner.remoteWords(at: 1)))
    }

    func testAThirdRefreshWithinFiveSecondsWaits() async throws {
        let cursor = makeCursor(["p1", "p2", "p3"])
        let session = makeSession(consumer: RecordingConsumer(cursor: cursor))
        await startAndWait(session)
        await session.refresh(projectIds: ["p1"])
        await settle()
        clock.advance(by: 1)
        await eventually("first restart") { self.runner.startCount == 2 }
        await settle()
        await session.refresh(projectIds: ["p2"])
        await settle()
        clock.advance(by: 1)
        await settle()
        XCTAssertEqual(runner.startCount, 2)
        clock.advance(by: 3.9)
        await settle()
        XCTAssertEqual(runner.startCount, 2)
        clock.advance(by: 0.1)
        await eventually("second restart") { self.runner.startCount == 3 }
    }

    // MARK: Stop and restart

    func testStopTerminatesTheChildAndNothingRestarts() async throws {
        let session = makeSession()
        await startAndWait(session)
        await session.stop()
        XCTAssertEqual(runner.child(at: 0).terminateCount, 1)
        clock.advance(by: 1000)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
        let state = await consumer.lastState()
        XCTAssertEqual(state, .stopped(.user))
    }

    func testStopDuringBackoffCancelsTheRetry() async throws {
        runner.enqueue(FakeScript(exit: 255))
        let session = makeSession()
        await startAndWait(session)
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        await session.stop()
        clock.advance(by: 1000)
        await settle()
        XCTAssertEqual(runner.startCount, 1)
    }

    func testRestartConnectsAtOnceAfterARepeatedFailureStop() async throws {
        for _ in 0..<5 { runner.enqueue(FakeScript(exit: 1)) }
        let session = makeSession()
        await startAndWait(session)
        for (index, delay) in [1.0, 2.0, 4.0, 8.0].enumerated() {
            await waitForState("attempt \(index + 1)") { self.isReconnecting($0, attempt: index + 1) }
            clock.advance(by: delay)
            await eventually("start \(index + 2)") { self.runner.startCount == index + 2 }
        }
        await waitForState("stopped") { $0 == .stopped(.repeatedFailure) }
        await session.restart()
        await eventually("sixth start") { self.runner.startCount == 6 }
    }

    func testRestartSkipsTheBackoffWait() async throws {
        runner.enqueue(FakeScript(exit: 255))
        let session = makeSession()
        await startAndWait(session)
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        await session.restart()
        await eventually("second start") { self.runner.startCount == 2 }
    }

    func testStartFailureCountsAsAFailureAndRetries() async throws {
        runner.failNextStart()
        let session = makeSession()
        await session.start()
        await waitForState("attempt 1") { self.isReconnecting($0, attempt: 1) }
        clock.advance(by: 1)
        await eventually("start") { self.runner.startCount == 1 }
    }
}
