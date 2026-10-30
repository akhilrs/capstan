import Foundation
import XCTest
@testable import CapstanKit

final class RecentHistoryTests: XCTestCase {
    private let epoch = RecentSamples.epoch
    private let t0 = Date(timeIntervalSince1970: 1_000_000)
    private let t1 = Date(timeIntervalSince1970: 1_000_060)
    private let shaA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    private let shaB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

    private func state() -> RecentHistoryState {
        RecentHistoryState(projectId: "pfixture")
    }

    private func backfilled(_ seqs: [Int], head: Int64 = 9) throws -> RecentHistoryState {
        var history = state()
        history.begin(epoch: epoch, head: head)
        let events: [FeedEvent] = try seqs.map { try RecentSamples.feedEvent(seq: $0) }
        history.apply(.page(RecentSamples.page(events, head: head)), at: t0)
        return history
    }

    private func sha(_ entry: HistoryEvent) -> JSONValue? {
        entry.event.body.raw["commitSha"]
    }

    // MARK: Store

    func testAnAnswerFillsTheStoreNewestFirst() throws {
        let history = try backfilled([7, 9, 8])
        XCTAssertEqual(history.events.map { $0.key.seq }, [9, 8, 7])
        XCTAssertEqual(history.phase, .backfilled)
        XCTAssertFalse(history.truncated)
        XCTAssertEqual(history.events.first?.source, .history)
    }

    func testALiveEventWithTheSameKeyIsNotShownTwice() throws {
        var history = state()
        history.begin(epoch: epoch, head: 9)
        history.merge(live: [try RecentSamples.feedEvent(seq: 9)], epoch: epoch, at: t0)
        history.apply(
            .page(RecentSamples.page([try RecentSamples.feedEvent(seq: 9), try RecentSamples.feedEvent(seq: 8)])),
            at: t1)
        XCTAssertEqual(history.events.map { $0.key.seq }, [9, 8])
        XCTAssertEqual(history.events.first?.source, .live)
    }

    func testTheStorePassesItsHeadAndLiveEventsAboveItJoinByTheSameKey() throws {
        var history = try backfilled([9, 8], head: 9)
        XCTAssertEqual(history.head, 9)
        XCTAssertEqual(history.epoch, epoch)
        history.merge(live: [try RecentSamples.feedEvent(seq: 10)], epoch: epoch, at: t1)
        XCTAssertEqual(history.events.map { $0.key.seq }, [10, 9, 8])
        history.merge(live: [try RecentSamples.feedEvent(seq: 10)], epoch: epoch, at: t1)
        XCTAssertEqual(history.events.count, 3)
        history.merge(live: [try RecentSamples.feedEvent(seq: 9)], epoch: epoch, at: t1)
        XCTAssertEqual(history.events.count, 3)
        XCTAssertEqual(history.events.map { $0.source }, [.live, .live, .history])
    }

    func testTheLiveCopyWinsWhenTheBodiesDifferAndTheHistoryCopyIsDropped() throws {
        var liveSecond = state()
        liveSecond.begin(epoch: epoch, head: 9)
        liveSecond.apply(.page(RecentSamples.page([try RecentSamples.feedEvent(seq: 9, sha: shaB)])), at: t0)
        liveSecond.merge(live: [try RecentSamples.feedEvent(seq: 9, sha: shaA)], epoch: epoch, at: t1)
        XCTAssertEqual(liveSecond.events.count, 1)
        XCTAssertEqual(sha(liveSecond.events[0]), .string(shaA))

        var liveFirst = state()
        liveFirst.begin(epoch: epoch, head: 9)
        liveFirst.merge(live: [try RecentSamples.feedEvent(seq: 9, sha: shaA)], epoch: epoch, at: t0)
        liveFirst.apply(.page(RecentSamples.page([try RecentSamples.feedEvent(seq: 9, sha: shaB)])), at: t1)
        XCTAssertEqual(liveFirst.events.count, 1)
        XCTAssertEqual(sha(liveFirst.events[0]), .string(shaA))
        XCTAssertEqual(liveFirst.events[0].source, .live)
    }

    func testAnEventThatIsNotAnOutcomeIsNotKept() throws {
        var history = state()
        history.merge(live: [try RecentSamples.feedEvent(seq: 3, type: "agent.registered")], epoch: epoch, at: t0)
        XCTAssertTrue(history.events.isEmpty)
    }

    func testUnknownCommandGivesSessionOnlyAndLiveEventsStillShow() throws {
        var history = state()
        history.begin(epoch: epoch, head: 9)
        history.apply(.sessionOnly, at: t0)
        XCTAssertEqual(history.phase, .sessionOnly)
        history.merge(live: [try RecentSamples.feedEvent(seq: 10)], epoch: epoch, at: t1)
        XCTAssertEqual(history.events.map { $0.key.seq }, [10])
    }

    func testAResyncDropsTheStoreAndShowsTheGapUntilTheNextReadSucceeds() throws {
        var history = try backfilled([9, 8])
        history.resync(at: t1)
        XCTAssertTrue(history.events.isEmpty)
        XCTAssertEqual(history.gapSince, t1)
        XCTAssertNil(history.epoch)
        // The answer of a read that began before the resync is not shown.
        history.apply(.page(RecentSamples.page([try RecentSamples.feedEvent(seq: 9)])), at: t1)
        XCTAssertTrue(history.events.isEmpty)
        history.begin(epoch: epoch, head: 12)
        history.apply(.page(RecentSamples.page([try RecentSamples.feedEvent(seq: 12)], head: 12)), at: t1)
        XCTAssertNil(history.gapSince)
        XCTAssertEqual(history.events.map { $0.key.seq }, [12])
    }

    func testAForeignPageLeavesLoadingSoARefreshCanStart() throws {
        var history = state()
        history.begin(epoch: epoch, head: 9)
        history.resync(at: t0)
        XCTAssertEqual(history.phase, .loading)
        history.apply(.page(RecentSamples.page([try RecentSamples.feedEvent(seq: 9)])), at: t1)
        XCTAssertEqual(history.phase, .idle)
    }

    func testAChangeOfEpochDropsTheStore() throws {
        var history = try backfilled([9, 8])
        history.begin(epoch: "0123456789abcdef", head: 3)
        XCTAssertTrue(history.events.isEmpty)
        let changed = try RecentSamples.feedEvent(seq: 4)
        history.merge(live: [changed], epoch: "fedcba9876543210", at: t1)
        XCTAssertEqual(history.events.count, 1)
        XCTAssertEqual(history.epoch, "fedcba9876543210")
    }

    func testARateLimitedAnswerKeepsTheStoreAndWaitsForTheWindow() throws {
        var history = try backfilled([9, 8])
        history.apply(.rateLimited, at: t1)
        XCTAssertEqual(history.events.count, 2)
        guard case .error = history.phase else { return XCTFail("\(history.phase)") }
        XCTAssertEqual(history.retryNotBefore, t1.addingTimeInterval(RecentCall.rateLimitWindowSeconds))
    }

    func testAnErrorKeepsWhatIsShown() throws {
        var history = try backfilled([9])
        history.apply(.error(code: "not_answering", message: "no"), at: t1)
        XCTAssertEqual(history.events.count, 1)
        XCTAssertEqual(history.phase, .error("Couldn't load earlier history"))
    }

    func testTruncatedIsKept() throws {
        var history = state()
        history.begin(epoch: epoch, head: 9)
        history.apply(.page(RecentSamples.page([], truncated: true)), at: t0)
        XCTAssertTrue(history.truncated)
        XCTAssertTrue(history.events.isEmpty)
    }

    // MARK: Trigger

    private func position(_ projectId: String) -> RecentPosition? {
        RecentPosition(epoch: epoch, head: 26)
    }

    private func call(_ projectId: String = "proj-a", head: Int64 = 26) -> RecentCall {
        RecentCall(projectId: projectId, epoch: epoch, head: head)
    }

    func testTheFirstSelectionAfterConnectCallsOnce() {
        var trigger = RecentTrigger()
        let host = UUID()
        XCTAssertEqual(trigger.handle(.connected(hostId: host), position: position), [])
        XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [call()])
        XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [])
    }

    func testASelectionBeforeTheConnectionCallsWhenItConnects() {
        var trigger = RecentTrigger()
        XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [])
        XCTAssertEqual(trigger.handle(.connected(hostId: UUID()), position: position), [call()])
    }

    func testAProjectWithoutAPositionIsReadWhenItsSnapshotArrives() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        let none = trigger.handle(.selected(projectId: "proj-a")) { (_: String) -> RecentPosition? in nil }
        XCTAssertEqual(none, [])
        XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [call()])
    }

    func testRefreshCalls() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        XCTAssertEqual(trigger.handle(.refreshed(projectId: "proj-a"), position: position), [call()])
        XCTAssertEqual(trigger.handle(.refreshed(projectId: "proj-b"), position: position), [])
    }

    func testAResyncCallsAtTheNewHead() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        XCTAssertEqual(
            trigger.handle(.resynced(projectId: "proj-a", head: 40), position: position), [call(head: 40)])
    }

    func testAnAnswerAndALaterSelectionOfTheSameProjectNeverCallAgain() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        let results: [RecentResult] = [.rateLimited, .error(code: "x", message: "y"), .sessionOnly]
        for result in results {
            XCTAssertEqual(trigger.handle(.answered(projectId: "proj-a", result), position: position), [])
        }
        for _ in 0..<5 {
            XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [])
        }
    }

    func testAHostWithoutTheCommandIsNotAskedAgainUntilRefresh() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        _ = trigger.handle(.answered(projectId: "proj-a", .sessionOnly), position: position)
        XCTAssertEqual(trigger.handle(.resynced(projectId: "proj-a", head: 30), position: position), [])
        XCTAssertEqual(trigger.handle(.refreshed(projectId: "proj-a"), position: position), [call()])
    }

    func testRefreshDoesNothingUntilTheRateLimitWindowEnds() {
        var trigger = RecentTrigger()
        _ = trigger.handle(.connected(hostId: UUID()), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        trigger.limit(projectId: "proj-a", until: t1)
        XCTAssertEqual(trigger.handle(.refreshed(projectId: "proj-a"), now: t0, position: position), [])
        XCTAssertEqual(trigger.handle(.refreshed(projectId: "proj-a"), now: t1, position: position), [call()])
    }

    func testANewConnectionReadsAgain() {
        var trigger = RecentTrigger()
        let host = UUID()
        _ = trigger.handle(.connected(hostId: host), position: position)
        _ = trigger.handle(.selected(projectId: "proj-a"), position: position)
        _ = trigger.handle(.disconnected(hostId: host), position: position)
        XCTAssertEqual(trigger.handle(.selected(projectId: "proj-a"), position: position), [])
        XCTAssertEqual(trigger.handle(.connected(hostId: host), position: position), [call()])
    }

    // MARK: Controller

    @MainActor
    func testNothingTheControllerReadsReachesTheReducerTheNotifyBaselineOrTheCursor() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder)
        let lines: [StreamLine] = [
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1")),
            try StoreWire.end("proj-a"),
        ]
        await engine.apply(host: hostId, lines: lines)
        let before = await engine.current()
        let changesBefore = recorder.changes

        let runner = FakeRunner()
        let events: [String] = [RecentSamples.event(seq: 27), RecentSamples.event(seq: 26)]
        runner.enqueue(FakeScript(exit: 0, stdout: RecentSamples.page(events: events, head: 26, projectId: "proj-a") + "\n"))
        let controller = OverviewController(store: engine, runner: runner, config: makeConfig(), clock: TestClock())
        await controller.update(host: before)
        await controller.handle(.connected(hostId: hostId))
        await controller.handle(.selected(projectId: "proj-a"))

        XCTAssertEqual(runner.startCount, 1)
        let held = controller.history("proj-a")
        XCTAssertEqual(held.phase, .backfilled)
        XCTAssertEqual(held.events.count, 2)
        let after = await engine.current()
        XCTAssertEqual(after, before)
        XCTAssertEqual(after.cursor, before.cursor)
        XCTAssertTrue(recorder.notifications.isEmpty)
        XCTAssertTrue(recorder.refreshes.isEmpty)
        XCTAssertEqual(recorder.changes, changesBefore)
    }

    @MainActor
    func testARateLimitedAnswerKeepsTheControllersStore() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        let lines: [StreamLine] = [
            try StoreWire.hello(), try StoreWire.project("proj-a"), try StoreWire.end("proj-a"),
        ]
        await engine.apply(host: hostId, lines: lines)
        let runner = FakeRunner()
        let page = RecentSamples.page(events: [RecentSamples.event(seq: 26)], head: 26, projectId: "proj-a")
        runner.enqueue(FakeScript(exit: 0, stdout: page + "\n"))
        let limited = "{\"v\":1,\"error\":{\"code\":\"rejected\",\"message\":\"rate_limited: slow down\"}}\n"
        runner.enqueue(FakeScript(exit: 1, stdout: limited))
        let controller = OverviewController(store: engine, runner: runner, config: makeConfig(), clock: TestClock())
        await controller.update(host: await engine.current())
        await controller.handle(.connected(hostId: hostId))
        await controller.handle(.selected(projectId: "proj-a"))
        XCTAssertEqual(controller.history("proj-a").events.count, 1)
        await controller.handle(.refreshed(projectId: "proj-a"))
        XCTAssertEqual(runner.startCount, 2)
        let held = controller.history("proj-a")
        XCTAssertEqual(held.events.count, 1)
        guard case .error = held.phase else { return XCTFail("\(held.phase)") }
        XCTAssertNotNil(held.retryNotBefore)
    }
}
