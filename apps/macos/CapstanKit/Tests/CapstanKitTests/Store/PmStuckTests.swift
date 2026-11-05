import Foundation
import XCTest
@testable import CapstanKit

/// The stuck episode of the PM: its events, the candidates they give and the run item that holds the open episode.
final class PmStuckTests: XCTestCase {
    private func stuckEvent(_ episode: String, alert: Bool, seq: Int) throws -> StreamLine {
        .event(projectId: "proj-a", try StoreWire.event(
            "pm.stuck", entity: ("agent", "pm-1"), seq: seq, effects: [("run", "invalidate", "run", "{}")],
            extraBody: ",\"agentId\":\"pm-1\",\"reason\":\"unread_mail\",\"since\":\"\(StoreWire.since)\","
                + "\"minutes\":15,\"episodeId\":\"\(episode)\",\"alert\":\(alert)"))
    }

    private func recoveredEvent(_ episode: String, seq: Int) throws -> StreamLine {
        .event(projectId: "proj-a", try StoreWire.event(
            "pm.recovered", entity: ("agent", "pm-1"), seq: seq, effects: [("run", "invalidate", "run", "{}")],
            extraBody: ",\"agentId\":\"pm-1\",\"episodeId\":\"\(episode)\",\"reason\":\"recovered\""))
    }

    private func runItem(stuck episode: String?, alert: Bool = true) -> String {
        let stuck = episode.map {
            "{\"episodeId\":\"\($0)\",\"reason\":\"unread_mail\",\"since\":\"\(StoreWire.since)\",\"alert\":\(alert)}"
        } ?? "null"
        return "{\"requestId\":\"run\",\"state\":\"active\",\"paused\":false,\"contentHash\":\"h\","
            + "\"pm\":{\"agentId\":\"pm-1\",\"waitingOn\":[],\"fromUser\":null,\"stuck\":\(stuck)}}"
    }

    private func seeded(stuck episode: String? = nil) throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: episode)),
            try StoreWire.end("proj-a"), try StoreWire.cursor(makeCursor(["proj-a"])))
        return harness
    }

    private func reconnect(_ harness: inout ReducerHarness, stuck: String? = nil) throws {
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: stuck)),
            try StoreWire.end("proj-a"))
    }

    private func topics(_ output: ReduceOutput) -> [NotifyTopic] { output.notify.map(\.topic) }

    // MARK: Events

    func testKernelFixturesDecode() throws {
        let stuck = try XCTUnwrap(try KernelFixtures.event("pm.stuck").body.pmStuckBody)
        XCTAssertEqual(stuck.agentId, "pm-1")
        XCTAssertEqual(stuck.reason, "unread_mail")
        XCTAssertEqual(stuck.minutes, 15)
        XCTAssertTrue(stuck.alert)
        XCTAssertTrue(stuck.episodeId.hasPrefix("stuck-"))
        XCTAssertNotNil(stuck.since)
        let recovered = try XCTUnwrap(try KernelFixtures.event("pm.recovered").body.pmRecoveredBody)
        XCTAssertEqual(recovered.episodeId, stuck.episodeId)
        XCTAssertEqual(recovered.reason, "recovered")
        XCTAssertNil(try KernelFixtures.event("pm.stuck").body.pmRecoveredBody)
    }

    func testAnUnknownReasonIsKeptAndABodyThatDoesNotReadIsNil() throws {
        let event = try StoreWire.event(
            "pm.stuck", entity: ("agent", "pm-1"), effects: [("run", "invalidate", "run", "{}")],
            extraBody: ",\"agentId\":\"pm-1\",\"reason\":\"later_reason\",\"episodeId\":\"e1\",\"alert\":true")
        XCTAssertEqual(event.body.pmStuckBody?.reason, "later_reason")
        XCTAssertNil(event.body.pmStuckBody?.minutes)
        let broken = try StoreWire.event(
            "pm.stuck", entity: ("agent", "pm-1"), effects: [("run", "invalidate", "run", "{}")], extraBody: ",\"agentId\":\"pm-1\"")
        XCTAssertNil(broken.body.pmStuckBody)
        XCTAssertEqual(broken.type, "pm.stuck", "the event itself still decodes")
    }

    func testARunItemWithAnAlertFalseStuckDecodes() throws {
        let item = try RunItem(json: try decodeJSON(Data(runItem(stuck: "e1", alert: false).utf8)))
        XCTAssertEqual(item.pm?.stuck?.episodeId, "e1")
        XCTAssertEqual(item.pm?.stuck?.alert, false)
    }

    // MARK: Candidates

    func testALiveAlertGivesOneCandidatePerEpisode() throws {
        var harness = try seeded()
        let first = harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        XCTAssertEqual(topics(first), [.pmStuck(episodeId: "e1")])
        let candidate = try XCTUnwrap(first.notify.first)
        XCTAssertEqual(candidate.kind, .other("pm_stuck"))
        XCTAssertEqual(candidate.key, "e1")
        XCTAssertEqual(candidate.projectId, "proj-a")
        let again = harness.apply(try stuckEvent("e1", alert: true, seq: 31))
        XCTAssertEqual(topics(again), [], "the same episode is announced once")
        let other = harness.apply(try stuckEvent("e2", alert: true, seq: 32))
        XCTAssertEqual(topics(other), [.pmStuck(episodeId: "e2")])
    }

    func testAlertFalseGivesNoCandidate() throws {
        var harness = try seeded()
        XCTAssertEqual(topics(harness.apply(try stuckEvent("e1", alert: false, seq: 30))), [])
        XCTAssertEqual(topics(harness.apply(try recoveredEvent("e1", seq: 31))), [], "nothing announced, nothing to withdraw")
    }

    func testASnapshotNeverGivesAStuckCandidate() throws {
        var harness = ReducerHarness()
        let first = harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: "e1")),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(first.notify, [])
        let second = harness.apply(
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: "e2")),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(second.notify, [])
    }

    func testAnEventBeforeTheFirstSnapshotOrCoveredByTheHeldOneGivesNothing() throws {
        var bare = ReducerHarness()
        bare.apply(try StoreWire.hello(), try StoreWire.project("proj-a"))
        XCTAssertEqual(topics(bare.apply(try stuckEvent("e1", alert: true, seq: 30))), [])
        var harness = try seeded()
        XCTAssertEqual(topics(harness.apply(try stuckEvent("e1", alert: true, seq: 20))), [], "seq 20 is below the head 26")
    }

    func testARecoveryEventGivesARecoveredCandidateOnce() throws {
        var harness = try seeded()
        harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        XCTAssertEqual(topics(harness.apply(try recoveredEvent("e1", seq: 31))), [.pmRecovered(episodeId: "e1")])
        XCTAssertEqual(topics(harness.apply(try recoveredEvent("e1", seq: 32))), [])
    }

    func testASnapshotThatDropsAnAnnouncedEpisodeGivesRecovered() throws {
        var harness = try seeded()
        harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        let output = harness.apply(
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: nil)),
            try StoreWire.end("proj-a", head: 40))
        XCTAssertEqual(topics(output), [.pmRecovered(episodeId: "e1")])
        let after = harness.apply(
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: nil)),
            try StoreWire.end("proj-a", head: 41))
        XCTAssertEqual(topics(after), [], "withdrawn once")
    }

    func testASnapshotThatStillHoldsTheEpisodeKeepsItAnnounced() throws {
        var harness = try seeded()
        harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        let same = harness.apply(
            try StoreWire.snapshot("proj-a", "run", key: "run", item: runItem(stuck: "e1")),
            try StoreWire.end("proj-a", head: 40))
        XCTAssertEqual(topics(same), [])
        XCTAssertEqual(topics(harness.apply(try recoveredEvent("e1", seq: 41))), [.pmRecovered(episodeId: "e1")])
    }

    func testWaitingCandidatesKeepTheWaitingTopic() throws {
        var harness = try seeded()
        let output = harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")),
            try StoreWire.end("proj-a", head: 40))
        XCTAssertEqual(topics(output), [.waiting])
    }

    // MARK: Catch-up

    func testACatchUpStuckEventNeverAnnouncesButALiveOneAfterTheCursorDoes() throws {
        var harness = try seeded()
        try reconnect(&harness)
        let catchUp = harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        XCTAssertEqual(topics(catchUp), [])
        harness.apply(try StoreWire.cursor(makeCursor(["proj-a"])))
        let live = harness.apply(try stuckEvent("e2", alert: true, seq: 31))
        XCTAssertEqual(topics(live), [.pmStuck(episodeId: "e2")])
    }

    func testACatchUpEpisodeThatOpenedAndRecoveredGivesNothing() throws {
        var harness = try seeded()
        try reconnect(&harness)
        let output = harness.apply(try stuckEvent("e1", alert: true, seq: 30), try recoveredEvent("e1", seq: 31))
        XCTAssertEqual(output.notify, [])
    }

    func testACatchUpRecoveryWithdrawsAnEpisodeAnnouncedBeforeTheDisconnect() throws {
        var harness = try seeded()
        harness.apply(try stuckEvent("e1", alert: true, seq: 30))
        try reconnect(&harness, stuck: "e1")
        XCTAssertEqual(topics(harness.apply(try recoveredEvent("e1", seq: 31))), [.pmRecovered(episodeId: "e1")])
    }

    func testASecondHelloStartsCatchUpAgain() throws {
        var harness = try seeded()
        try reconnect(&harness)
        harness.apply(try StoreWire.cursor(makeCursor(["proj-a"])))
        try reconnect(&harness)
        XCTAssertEqual(topics(harness.apply(try stuckEvent("e1", alert: true, seq: 30))), [])
        harness.apply(try StoreWire.cursor(makeCursor(["proj-a"])))
        XCTAssertEqual(topics(harness.apply(try stuckEvent("e1", alert: true, seq: 31))), [.pmStuck(episodeId: "e1")])
    }
}
