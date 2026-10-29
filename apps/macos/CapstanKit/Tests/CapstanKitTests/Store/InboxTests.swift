import Foundation
import XCTest
@testable import CapstanKit

final class InboxTests: XCTestCase {
    private func host() throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:1", item: StoreWire.waitingItem("integration:1")),
            try StoreWire.snapshot("proj-a", "waiting", key: "message:4", item: StoreWire.waitingItem("message:4", since: "2026-10-01T10:00:00.000Z")),
            try StoreWire.snapshot("proj-a", "waiting", key: "message:2", item: StoreWire.waitingItem("message:2")),
            try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")),
            try StoreWire.snapshot("proj-a", "waiting", key: "design:1/p", item: StoreWire.waitingItem("design:1/p")),
            try StoreWire.snapshot("proj-a", "waiting", key: "prompt:3", item: StoreWire.waitingItem("prompt:3")),
            try StoreWire.snapshot("proj-a", "waiting", key: "extra:1", item: StoreWire.waitingItem("extra:1")),
            try StoreWire.end("proj-a"))
        return harness
    }

    func testGroupsComeInOrderWithCountsAndTitles() throws {
        let groups = inbox([try host().host])
        XCTAssertEqual(groups.map(\.title), ["Command proposals", "Prompts", "Designs", "Messages", "Ready to merge"])
        XCTAssertEqual(groups.map(\.count), [1, 1, 1, 2, 1])
        XCTAssertEqual(groups[3].rows.map(\.id.key), ["message:2", "message:4"], "oldest first")
        XCTAssertEqual(badgeCount([try host().host]), 6)
    }

    func testRowFields() throws {
        let harness = try host()
        let row = try XCTUnwrap(inbox([harness.host]).first?.rows.first)
        XCTAssertEqual(row.id, InboxRowID(hostId: harness.host.hostId, projectId: "proj-a", key: "op:7"))
        XCTAssertEqual(row.projectName, "proj-a")
        XCTAssertEqual(row.since, StoreWire.since)
        XCTAssertEqual(row.summary.raw, "run ls")
        XCTAssertFalse(row.needsTerminal)
        XCTAssertFalse(row.forwarded)
        XCTAssertFalse(row.stale)
    }

    func testFilter() throws {
        let harness = try host()
        XCTAssertEqual(inbox([harness.host], filter: InboxFilter(kind: .message)).count, 1)
        XCTAssertEqual(inbox([harness.host], filter: InboxFilter(projectId: "proj-z")), [])
        XCTAssertEqual(inbox([harness.host], filter: InboxFilter(hostId: UUID())), [])
    }

    func testForwardedWhileTheItemKeepsItsHash() throws {
        var harness = try host()
        let record = ForwardRecord(requestId: "req-op:7", contentHash: "hash-1", messageId: "m", at: Date())
        harness.host.forwards["proj-a"] = [record]
        XCTAssertEqual(inbox([harness.host]).first?.rows.first?.forwarded, true)
        XCTAssertEqual(badgeCount([harness.host]), 5)
        harness.host.forwards["proj-a"] = [ForwardRecord(requestId: "req-op:7", contentHash: "other", at: Date())]
        XCTAssertEqual(inbox([harness.host]).first?.rows.first?.forwarded, false)
    }

    func testRowsAreOrderedBySinceAsADateNotAsText() throws {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "op:1", item: StoreWire.waitingItem("op:1", since: "2026-10-01T09:30:00.500Z")),
            try StoreWire.snapshot("proj-a", "waiting", key: "op:2", item: StoreWire.waitingItem("op:2", since: "2026-10-01T09:30:00Z")),
            try StoreWire.end("proj-a"))
        // As text "…00.500Z" sorts before "…00Z"; as dates 00Z is the older one.
        XCTAssertEqual(inbox([harness.host]).first?.rows.map(\.id.key), ["op:2", "op:1"])
    }

    func testRowsOfAProjectThatIsNotReadyAreStale() throws {
        var harness = try host()
        harness.apply(try StoreWire.project("proj-a", state: "silent"))
        XCTAssertTrue(inbox([harness.host]).allSatisfy { $0.rows.allSatisfy(\.stale) })
    }
}
