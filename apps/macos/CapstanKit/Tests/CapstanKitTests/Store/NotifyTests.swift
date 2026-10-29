import Foundation
import XCTest
@testable import CapstanKit

final class NotifyTests: XCTestCase {
    private func lines(_ keys: [String]) throws -> [StreamLine] {
        try keys.map { try StoreWire.snapshot("proj-a", "waiting", key: $0, item: StoreWire.waitingItem($0)) }
            + [try StoreWire.end("proj-a")]
    }

    private func started() throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(try StoreWire.hello(), try StoreWire.project("proj-a"))
        return harness
    }

    func testTheFirstCompleteSnapshotNotifiesNothing() throws {
        var harness = try started()
        XCTAssertTrue(harness.apply(try lines(["op:1", "message:2"])).notify.isEmpty)
    }

    func testARefreshSnapshotThatAddsAKeyNotifiesOnlyThatKey() throws {
        var harness = try started()
        harness.apply(try lines(["message:2"]))
        let output = harness.apply(try lines(["message:2", "op:7"]))
        XCTAssertEqual(output.notify.map(\.key), ["op:7"])
        XCTAssertEqual(output.notify.first?.kind, .op)
        XCTAssertEqual(output.notify.first?.projectId, "proj-a")
        XCTAssertEqual(output.notify.first?.hostId, harness.host.hostId)
    }

    func testAResyncSnapshotWithTheSameKeysNotifiesNothing() throws {
        var harness = try started()
        harness.apply(try lines(["op:7"]))
        harness.apply(try StoreWire.resync("proj-a"))
        XCTAssertTrue(harness.apply(try lines(["op:7"])).notify.isEmpty)
        harness.apply(try StoreWire.resync(nil))
        XCTAssertEqual(harness.apply(try lines(["op:7", "op:8"])).notify.map(\.key), ["op:8"])
    }

    func testAnUpsertOfANewKeyNotifiesOnceAndOnlyAfterABaseline() throws {
        var harness = try started()
        let fields = "{\"requestId\":\"r\",\"kind\":\"op\",\"since\":\"\(StoreWire.since)\",\"summary\":\"go\","
            + "\"contentHash\":\"h\",\"needsTerminal\":false}"
        let event = try StoreWire.event("operator.proposed", effects: [("waiting", "upsert", "op:9", fields)])
        XCTAssertTrue(harness.apply(.event(projectId: "proj-a", event)).notify.isEmpty, "no baseline yet")
        harness.apply(try lines([]))
        let again = try StoreWire.event("operator.proposed", effects: [("waiting", "upsert", "op:10", fields)])
        XCTAssertEqual(harness.apply(.event(projectId: "proj-a", again)).notify.map(\.key), ["op:10"])
        XCTAssertTrue(harness.apply(.event(projectId: "proj-a", again)).notify.isEmpty)
        XCTAssertTrue(harness.apply(try lines(["op:10"])).notify.isEmpty)
    }

    func testAfterDropCursorOnlyANewKeyNotifies() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder)
        await engine.apply(host: hostId, lines: [try StoreWire.hello(), try StoreWire.project("proj-a")] + (try lines([])))
        await engine.apply(host: hostId, lines: try lines(["op:7"]))
        XCTAssertEqual(recorder.notifications.map(\.key), ["op:7"])
        await engine.dropCursor(host: hostId)
        await engine.apply(host: hostId, lines: try lines(["op:7", "op:8"]))
        XCTAssertEqual(recorder.notifications.map(\.key), ["op:7", "op:8"])
    }
}
