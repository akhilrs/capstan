import Foundation
import XCTest
@testable import CapstanKit

/// A monotonic clock a test moves by hand.
final class FakeMonotonic: MonotonicClock, @unchecked Sendable {
    private let lock = NSLock()
    private var current: TimeInterval = 0

    func seconds() -> TimeInterval {
        lock.withLock { current }
    }

    func advance(by delta: TimeInterval) {
        lock.withLock { current += delta }
    }
}

final class FreshnessTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000_000)
    private let t1 = Date(timeIntervalSince1970: 1_000_100)
    private let t2 = Date(timeIntervalSince1970: 1_000_200)
    private let t3 = Date(timeIntervalSince1970: 1_000_300)

    private func snapshotLines() throws -> [StreamLine] {
        [
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1")),
            try StoreWire.end("proj-a"),
        ]
    }

    private func invalidate(_ section: String, _ key: String, seq: Int = 40) throws -> StreamLine {
        .event(
            projectId: "proj-a",
            try StoreWire.event("snapshot.invalidated", seq: seq, effects: [(section, "invalidate", key, "{}")]))
    }

    private func freshness(_ harness: ReducerHarness) throws -> ProjectFreshness {
        ProjectFreshness(project: try XCTUnwrap(harness.alpha), liveness: .live)
    }

    func testRefreshingFromTheFirstSnapshotLineToSnapshotEnd() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")], now: t0)
        let first = try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1"))
        harness.apply(first, now: t0)
        XCTAssertTrue(try freshness(harness).isRefreshing)
        XCTAssertNil(try freshness(harness).currentAsOf)
        harness.apply(try StoreWire.end("proj-a"), now: t1)
        XCTAssertFalse(try freshness(harness).isRefreshing)
        XCTAssertEqual(try freshness(harness).currentAsOf, t1)
    }

    func testAnEmptySnapshotIsRefreshingUntilItsEnd() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")], now: t0)
        XCTAssertTrue(try freshness(harness).isRefreshing)
        harness.apply(try StoreWire.end("proj-a"), now: t1)
        XCTAssertFalse(try freshness(harness).isRefreshing)
        XCTAssertEqual(try freshness(harness).currentAsOf, t1)
    }

    func testAResyncIsRefreshingAgainAndKeepsWhatWasLastKnown() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")] + snapshotLines(), now: t1)
        harness.apply(try StoreWire.resync("proj-a"), now: t2)
        XCTAssertTrue(try freshness(harness).isRefreshing)
        XCTAssertEqual(try freshness(harness).lastKnownAt, t1)
    }

    func testAnInvalidateMarksOneKeyPendingAndCurrentAsOfWaitsForItsReread() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")] + snapshotLines(), now: t1)
        harness.apply(try invalidate("agents", "dev-1"), now: t2)
        let during = try freshness(harness)
        XCTAssertEqual(during.pendingKeys, ["agents/dev-1"])
        XCTAssertEqual(during.currentAsOf, t1)
        harness.apply(try snapshotLines(), now: t3)
        let after = try freshness(harness)
        XCTAssertEqual(after.pendingKeys, [])
        XCTAssertEqual(after.currentAsOf, t3)
    }

    func testAnEventWithNothingPendingMovesCurrentAsOf() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")] + snapshotLines(), now: t1)
        let event = try StoreWire.event(
            "agent.changed", seq: 41, effects: [("agents", "upsert", "dev-1", "{\"state\":\"active\"}")])
        harness.apply(.event(projectId: "proj-a", event), now: t2)
        XCTAssertEqual(try freshness(harness).currentAsOf, t2)
    }

    func testAHeartbeatNeverRefreshesCurrentAsOf() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")] + snapshotLines(), now: t1)
        let beat = try StoreWire.line("{\"v\":1,\"kind\":\"heartbeat\",\"ts\":\"2026-10-01T09:30:00.000Z\"}")
        harness.apply(beat, now: t3)
        XCTAssertEqual(try freshness(harness).currentAsOf, t1)
    }

    func testAFailedRereadSetsRefreshFailedAtAndKeepsLastKnownAt() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello(), StoreWire.project("proj-a")] + snapshotLines(), now: t1)
        harness.apply(try invalidate("agents", "dev-1"), now: t2)
        StoreReducer.noteRefreshFailure(in: &harness.host, now: t3)
        let failed = try freshness(harness)
        XCTAssertEqual(failed.refreshFailedAt, t3)
        XCTAssertEqual(failed.lastKnownAt, t1)
        harness.apply(try snapshotLines(), now: t3)
        XCTAssertNil(try freshness(harness).refreshFailedAt)
    }

    func testAnEngineNotesTheFailureWhenTheStreamEndsBeforeTheReadFinished() async throws {
        let hostId = UUID()
        let clock = TestClock()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), clock: clock)
        await engine.apply(host: hostId, lines: [try StoreWire.hello(), try StoreWire.project("proj-a")])
        await engine.apply(
            host: hostId,
            lines: [try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1"))])
        let state = ConnectionState.reconnecting(attempt: 1, retryAt: clock.now(), lastError: nil)
        await engine.connection(host: hostId, state)
        let model = await engine.current()
        XCTAssertEqual(model.projects["proj-a"]?.freshness?.refreshFailedAt, clock.now())
    }

    func testLivenessUsesTheMonotonicClockAndTheWallClockMovingBackwardsChangesNothing() async throws {
        let hostId = UUID()
        let wall = TestClock()
        let monotonic = FakeMonotonic()
        let recorder = EngineRecorder()
        let engine = HostStoreEngine(
            hostId: hostId, persistence: MemoryPersistence(), clock: wall,
            onChange: { recorder.change($0) }, onNotify: { recorder.notify($0) }, onRefresh: { recorder.refresh($0) },
            monotonic: monotonic)
        let beat = try StoreWire.line("{\"v\":1,\"kind\":\"heartbeat\",\"ts\":\"2026-10-01T09:30:00.000Z\"}")
        let before = await engine.connectionLiveness(intervalSeconds: 2)
        XCTAssertEqual(before, .unknown)
        await engine.apply(host: hostId, lines: [beat])
        monotonic.advance(by: 5)
        wall.advance(by: -100_000)
        let backwards = await engine.connectionLiveness(intervalSeconds: 2)
        XCTAssertEqual(backwards, .live)
        monotonic.advance(by: 2)
        let silent = await engine.connectionLiveness(intervalSeconds: 2)
        XCTAssertEqual(silent, .silent)
    }

    func testTheThreeIntervalRule() {
        var tracker = LivenessTracker()
        XCTAssertEqual(tracker.state(at: 0, intervalSeconds: 2), .unknown)
        tracker.noteLine(at: 10)
        XCTAssertEqual(tracker.state(at: 16, intervalSeconds: 2), .live)
        XCTAssertEqual(tracker.state(at: 16.5, intervalSeconds: 2), .silent)
        tracker.reset()
        XCTAssertEqual(tracker.state(at: 99, intervalSeconds: 2), .unknown)
    }

    func testFreshnessReadsNoWallClockForElapsedTime() throws {
        let path = RepoPaths.repoRoot.appendingPathComponent("apps/macos/CapstanKit/Sources/CapstanKit/Store/Freshness.swift")
        let source = try String(contentsOf: path, encoding: .utf8)
        XCTAssertFalse(source.contains("Date()"))
        XCTAssertFalse(source.contains("Date.now"))
        XCTAssertFalse(source.contains("timeIntervalSinceNow"))
    }

    func testAnOlderStoredProjectDecodesWithNoMarks() throws {
        let json = "{\"projectId\":\"p\",\"root\":\"/r\",\"availability\":\"ready\",\"waiting\":{},\"agents\":{},"
            + "\"plans\":{},\"details\":{},\"attention\":{},\"snapshotComplete\":true}"
        let project = try JSONDecoder().decode(ProjectModel.self, from: Data(json.utf8))
        XCTAssertNil(project.freshness)
        XCTAssertNil(project.foldedSections)
        XCTAssertEqual(project.folded, [])
    }
}
