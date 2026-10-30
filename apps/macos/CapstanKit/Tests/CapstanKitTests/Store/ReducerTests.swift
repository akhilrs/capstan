import Foundation
import XCTest
@testable import CapstanKit

final class ReducerTests: XCTestCase {
    private func seeded() throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1")),
            try StoreWire.end("proj-a"))
        return harness
    }

    /// The item at `index` of the page example of the protocol document that holds `marker`, as JSON text.
    private func docItemAt(_ marker: String, _ index: Int) throws -> String {
        let block = try XCTUnwrap(ProtocolDoc.jsonBlocks().first { $0.text.contains(marker) }, marker)
        let page = try decodeJSON(Data(block.text.utf8))
        let item = try XCTUnwrap(page["items"]?[index])
        return String(decoding: try JSONEncoder().encode(item), as: UTF8.self)
    }

    func testDocExamplesGiveOneMessagesGroupAnIntegrationAWorkingAgentAndAPlanAtZeroOfThree() throws {
        var harness = ReducerHarness()
        harness.apply(try docStreamStart())
        harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "message:3", item: try docItem("\"requestId\":\"message:msg-1\"")),
            try StoreWire.snapshot(
                "proj-a", "waiting", key: "integration:7", item: try docItemAt("\"requestId\":\"message:msg-1\"", 1)),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: try docItem("\"requestId\":\"agent:dev-1\"")),
            try StoreWire.snapshot("proj-a", "plans", key: "1", item: try docItem("\"planId\":\"plan-1\"")))
        XCTAssertEqual(inbox([harness.host]), [], "nothing is visible before snapshotEnd")
        harness.apply(try StoreWire.end("proj-a"))

        let groups = inbox([harness.host])
        XCTAssertEqual(groups.map { $0.title }, ["Messages", "Ready to merge"])
        XCTAssertEqual(groups.map { $0.count }, [1, 1])
        XCTAssertEqual(groups.first?.rows.first?.projectName, "alpha")
        let integration = try XCTUnwrap(harness.alpha?.waiting["integration:7"])
        XCTAssertEqual(integration.integrationState, "merged")
        XCTAssertEqual(integration.branch?.raw, "integration/plan-3")
        XCTAssertEqual(integration.integrationDetails?.recorded, true)
        XCTAssertEqual(integration.integrationDetails?.plans.first?.planId, "plan-3")
        XCTAssertEqual(integration.integrationDetails?.review?.verdict, "passed")
        XCTAssertNil(harness.alpha?.waiting["message:3"]?.integrationDetails)
        let project = try XCTUnwrap(harness.alpha)
        let agent = try XCTUnwrap(project.agents["dev-1"])
        // The doc's agent example carries activity "working" and no task.
        XCTAssertEqual(project.health(of: agent), .working)
        let plan = try XCTUnwrap(project.plans["1"])
        XCTAssertEqual(PlanProgress.label(plan), "0/3")
        XCTAssertEqual(PlanProgress(plan).integrated, 0)
        XCTAssertEqual(harness.host.hello?.host.raw, "build-host")
        XCTAssertEqual(harness.host.projects["proj-b"]?.availability, .silent)
        XCTAssertEqual(harness.host.projects["proj-b"]?.snapshotComplete, false)
    }

    func testSnapshotLinesWithoutEndChangeNothingVisible() throws {
        var harness = try seeded()
        // The first snapshot line starts the refresh; nothing else visible changes until the end mark.
        var expected = harness.host
        expected.projects["proj-a"]?.markFreshness { (marks: inout FreshnessMarks) -> Void in marks.refreshing = true }
        harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-2", item: StoreWire.agentItem("dev-2")))
        XCTAssertEqual(harness.host, expected)
        XCTAssertEqual(harness.host.projects["proj-a"]?.freshness?.refreshing, true)
        XCTAssertEqual(harness.staging.stagedProjectIds, ["proj-a"])
    }

    func testAProjectLineMidStreamAsksForNoRefresh() throws {
        var harness = try seeded()
        let output = harness.apply(try StoreWire.project("proj-a"), try StoreWire.project("proj-b", state: "silent"))
        XCTAssertTrue(output.refresh.isEmpty)
        XCTAssertEqual(harness.host.projects["proj-a"]?.snapshotComplete, true)
    }

    func testAgentBranchIsReadAndKeptWhenAnEffectDoesNotStateIt() throws {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot(
                "proj-a", "agents", key: "dev-1",
                item: StoreWire.agentItem("dev-1").replacingOccurrences(of: "\"requestId\"", with: "\"branch\":\"feat/x\",\"requestId\"")),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(harness.alpha?.agents["dev-1"]?.branch?.raw, "feat/x")
    }

    private func seededWithRun() throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "run", key: "run", item: StoreWire.runItem()),
            try StoreWire.end("proj-a"))
        return harness
    }

    func testRunChangedPausedUpdatesTheStoredRunAndAsksForTheRead() throws {
        var harness = try seededWithRun()
        XCTAssertEqual(harness.alpha?.run?.paused, false)
        let output = harness.apply(try StoreWire.runChanged(state: "paused", paused: true))
        XCTAssertEqual(harness.alpha?.run?.paused, true)
        XCTAssertEqual(harness.alpha?.run?.state, "paused")
        XCTAssertEqual(harness.alpha?.run?.requestId, "run")
        XCTAssertEqual(harness.alpha?.run?.hashKnown, false, "the old hash is dropped, not kept stale")
        XCTAssertEqual(output.refresh, ["proj-a"])
        XCTAssertEqual(output.urgent, ["proj-a"], "the person waits for the run to follow")
    }

    func testAnInvalidateOfAnAgentIsNotUrgent() throws {
        var harness = try seeded()
        let output = harness.apply(
            .event(projectId: "proj-a", try StoreWire.event(
                "snapshot.invalidated", effects: [("agents", "invalidate", "dev-1", "{}")])))
        XCTAssertEqual(output.refresh, ["proj-a"])
        XCTAssertTrue(output.urgent.isEmpty)
    }

    func testARemoteDecisionInvalidateIsUrgent() throws {
        var harness = try seeded()
        let output = harness.apply(
            .event(projectId: "proj-a", try StoreWire.event(
                "decision.remote", effects: [("agents", "invalidate", "dev-1", "{}")])))
        XCTAssertEqual(output.urgent, ["proj-a"])
    }

    func testRunChangedResumeFlipsTheStoredRunBack() throws {
        var harness = try seededWithRun()
        harness.apply(try StoreWire.runChanged(state: "paused", paused: true))
        harness.apply(try StoreWire.runChanged(state: "active", paused: false, seq: 32))
        XCTAssertEqual(harness.alpha?.run?.paused, false)
        XCTAssertEqual(harness.alpha?.run?.state, "active")
    }

    func testTheReadAfterARunChangeBringsTheNewHash() throws {
        var harness = try seededWithRun()
        harness.apply(try StoreWire.runChanged(state: "paused", paused: true))
        harness.apply(
            try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "run", key: "run", item: StoreWire.runItem(state: "paused", paused: true, hash: "run-hash-2")),
            try StoreWire.end("proj-a", head: 31))
        XCTAssertEqual(harness.alpha?.run?.contentHash, "run-hash-2")
        XCTAssertEqual(harness.alpha?.run?.hashKnown, true)
        XCTAssertEqual(harness.alpha?.run?.paused, true)
    }

    func testRunChangedWithoutAStoredRunLeavesNoRun() throws {
        var harness = try seeded()
        harness.apply(try StoreWire.runChanged(state: "paused", paused: true))
        XCTAssertNil(harness.alpha?.run)
    }

    private func activityEvent(_ activity: String, seq: Int, key: String = "dev-1") throws -> StreamLine {
        .event(projectId: "proj-a", try StoreWire.event(
            "agent.activity", entity: ("agent", key),
            effects: [("agents", "upsert", key, "{\"agentId\":\"\(key)\",\"activity\":\"\(activity)\",\"activitySeq\":\(seq),\"activitySince\":\"2026-01-01T00:00:00.033Z\"}")]))
    }

    func testTheActivityFixtureSetsTheThreeFields() throws {
        var harness = try seeded()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("agent.activity")))
        let agent = try XCTUnwrap(harness.alpha?.agents["dev-1"])
        XCTAssertEqual(agent.activity, "working")
        XCTAssertEqual(agent.activitySeq, 1)
        XCTAssertEqual(agent.activitySince, "2026-01-01T00:00:00.033Z")
    }

    func testALowerActivitySeqChangesNothing() throws {
        var harness = try seeded()
        harness.apply(try activityEvent("working", seq: 5))
        let before = harness.alpha?.agents["dev-1"]
        harness.apply(try activityEvent("idle", seq: 4), try activityEvent("blocked", seq: 5))
        XCTAssertEqual(harness.alpha?.agents["dev-1"], before)
        harness.apply(try activityEvent("idle", seq: 6))
        XCTAssertEqual(harness.alpha?.agents["dev-1"]?.activity, "idle")
        XCTAssertEqual(harness.alpha?.agents["dev-1"]?.activitySeq, 6)
    }

    func testALaterUpsertWithoutTheFieldsKeepsThem() throws {
        var harness = try seeded()
        harness.apply(try activityEvent("working", seq: 2))
        harness.apply(.event(projectId: "proj-a", try StoreWire.event(
            "agent.paused", effects: [("agents", "upsert", "dev-1", "{\"state\":\"blocked\"}")])))
        let agent = try XCTUnwrap(harness.alpha?.agents["dev-1"])
        XCTAssertEqual(agent.state, "blocked")
        XCTAssertEqual(agent.activity, "working")
        XCTAssertEqual(agent.activitySeq, 2)
        XCTAssertEqual(agent.activitySince, "2026-01-01T00:00:00.033Z")
    }

    func testAnActivityUpsertForAnUnheldAgentChangesNothingAndAsksNoRefresh() throws {
        var harness = try seeded()
        let before = harness.alpha
        let output = harness.apply(try activityEvent("working", seq: 1, key: "dev-9"))
        XCTAssertNil(harness.alpha?.agents["dev-9"])
        XCTAssertEqual(harness.alpha?.agents, before?.agents)
        XCTAssertFalse(output.refresh.contains("proj-a"))
    }

    func testResyncDropsOneProjectAndNullDropsAll() throws {
        var harness = try seeded()
        harness.apply(
            try StoreWire.project("proj-b"),
            try StoreWire.snapshot("proj-b", "waiting", key: "op:1", item: StoreWire.waitingItem("op:1")),
            try StoreWire.end("proj-b"),
            .event(projectId: "proj-a", try StoreWire.event(
                "prompt.waiting", entity: ("prompt_relay", "relay-1"),
                effects: [("waiting", "invalidate", "prompt:1", "{}")])),
            .event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        XCTAssertFalse(try XCTUnwrap(harness.alpha).details.isEmpty)
        XCTAssertFalse(try XCTUnwrap(harness.alpha).attention.isEmpty)

        harness.apply(try StoreWire.resync("proj-a"))
        let alpha = try XCTUnwrap(harness.alpha)
        XCTAssertTrue(alpha.agents.isEmpty && alpha.waiting.isEmpty && alpha.details.isEmpty && alpha.attention.isEmpty)
        XCTAssertFalse(alpha.snapshotComplete)
        XCTAssertEqual(harness.host.projects["proj-b"]?.waiting.count, 1, "the other project is untouched")

        harness.apply(try StoreWire.resync(nil))
        XCTAssertTrue(harness.host.projects.values.allSatisfy { $0.waiting.isEmpty && !$0.snapshotComplete })
    }

    func testResyncDropsStagedLines() throws {
        var harness = try seeded()
        harness.apply(try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")))
        harness.apply(try StoreWire.resync("proj-a"), try StoreWire.end("proj-a"))
        XCTAssertTrue(try XCTUnwrap(harness.alpha).waiting.isEmpty)
    }

    func testProjectThatIsNotReadyKeepsItsItemsWithStaleSince() throws {
        var harness = try seeded()
        let at = Date(timeIntervalSince1970: 2_000_000)
        harness.apply(try StoreWire.project("proj-a", state: "silent"), now: at)
        var alpha = try XCTUnwrap(harness.alpha)
        XCTAssertEqual(alpha.availability, .silent)
        XCTAssertEqual(alpha.staleSince, at)
        XCTAssertNotNil(alpha.agents["dev-1"])
        harness.apply(try StoreWire.project("proj-a", state: "gone"), now: at.addingTimeInterval(60))
        XCTAssertEqual(try XCTUnwrap(harness.alpha).staleSince, at, "the first time stays")
        harness.apply(try StoreWire.project("proj-a", head: 40))
        alpha = try XCTUnwrap(harness.alpha)
        XCTAssertNil(alpha.staleSince)
        XCTAssertEqual(alpha.head, 40)
        XCTAssertEqual(alpha.epoch, "daeca3b2245f3519")
    }

    func testUpsertMergesIntoAnAgent() throws {
        var harness = try seeded()
        let event = try StoreWire.event(
            "agent.paused", effects: [("agents", "upsert", "dev-1", "{\"paused\":true,\"state\":\"paused\"}")])
        let output = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertTrue(output.refresh.isEmpty)
        let agent = try XCTUnwrap(harness.alpha?.agents["dev-1"])
        XCTAssertTrue(agent.paused)
        XCTAssertEqual(agent.state, "paused")
        XCTAssertEqual(agent.contentHash, "agent-hash", "other fields stay")
    }

    func testUpsertCreatesItemsFromFullFields() throws {
        var harness = try seeded()
        let agentFields = "{\"agentId\":\"dev-2\",\"kind\":\"Developer\",\"role\":\"developer\",\"generation\":1,"
            + "\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:dev-2\",\"contentHash\":\"h\"}"
        let waitingFields = "{\"requestId\":\"req\",\"kind\":\"op\",\"since\":\"\(StoreWire.since)\",\"summary\":\"go\","
            + "\"contentHash\":\"h\",\"needsTerminal\":true}"
        let planFields = "{\"planId\":\"plan-2\",\"sequence\":2,\"title\":\"Next\",\"state\":\"draft\","
            + "\"packages\":{\"total\":1,\"assigned\":0,\"reported\":0,\"integrated\":0}}"
        let event = try StoreWire.event("plan.changed", effects: [
            ("agents", "upsert", "dev-2", agentFields), ("waiting", "upsert", "op:9", waitingFields),
            ("plans", "upsert", "2", planFields),
        ])
        let output = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertTrue(output.refresh.isEmpty)
        let alpha = try XCTUnwrap(harness.alpha)
        XCTAssertEqual(alpha.agents["dev-2"]?.key, "dev-2")
        XCTAssertEqual(alpha.waiting["op:9"]?.kind, .op)
        XCTAssertEqual(alpha.waiting["op:9"]?.needsTerminal, true)
        XCTAssertEqual(alpha.plans["2"]?.title.raw, "Next")
    }

    func testWaitingUpsertWithoutContentHashCreatesNothingAndRefreshes() throws {
        var harness = try seeded()
        let fields = "{\"since\":\"\(StoreWire.since)\",\"summary\":\"go\",\"needsTerminal\":false}"
        let event = try StoreWire.event("operator.proposed", effects: [("waiting", "upsert", "op:9", fields)])
        let output = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertNil(harness.alpha?.waiting["op:9"])
        XCTAssertEqual(output.refresh, ["proj-a"])
        XCTAssertTrue(output.notify.isEmpty)
    }

    func testRemoveDeletesTheItemItsDetailAndItsAttention() throws {
        var harness = try seeded()
        harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")),
            try StoreWire.end("proj-a"),
            .event(projectId: "proj-a", try KernelFixtures.event("operator.proposed")),
            .event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        XCTAssertNotNil(harness.alpha?.details["op:1"])
        let event = try StoreWire.event("operator.decided", effects: [
            ("waiting", "remove", "op:7", "{}"), ("waiting", "remove", "op:1", "{}"), ("agents", "remove", "dev-1", "{}"),
        ])
        harness.apply(.event(projectId: "proj-a", event))
        let alpha = try XCTUnwrap(harness.alpha)
        XCTAssertNil(alpha.waiting["op:7"])
        XCTAssertNil(alpha.details["op:1"])
        XCTAssertNil(alpha.agents["dev-1"])
        XCTAssertNil(alpha.attention["dev-1"])
    }

    func testInvalidateOfAKeyAndOfABareKindAddsRefresh() throws {
        var harness = try seeded()
        let key = try StoreWire.event("agent.lost", effects: [("agents", "invalidate", "dev-1", "{}")])
        XCTAssertEqual(harness.apply(.event(projectId: "proj-a", key)).refresh, ["proj-a"])
        let kind = try StoreWire.event("message.state", effects: [("waiting", "invalidate", "message", "{}")])
        XCTAssertEqual(harness.apply(.event(projectId: "proj-a", kind)).refresh, ["proj-a"])
        XCTAssertTrue(try XCTUnwrap(harness.alpha).details.isEmpty, "a bare kind names no item")
    }

    func testAnUnknownOpOrSectionDropsTheProjectAndRefreshes() throws {
        var harness = try seeded()
        let move = try StoreWire.event("agent.registered", effects: [("agents", "move", "dev-1", "{}")])
        var output = harness.apply(.event(projectId: "proj-a", move))
        XCTAssertEqual(output.refresh, ["proj-a"])
        XCTAssertTrue(try XCTUnwrap(harness.alpha).agents.isEmpty)
        XCTAssertFalse(try XCTUnwrap(harness.alpha).snapshotComplete)

        harness.apply(try StoreWire.end("proj-a"))
        let section = try StoreWire.event("agent.registered", effects: [("widgets", "upsert", "w", "{}")])
        output = harness.apply(.event(projectId: "proj-a", section))
        XCTAssertEqual(output.refresh, ["proj-a"])
    }

    func testEffectsOfAnUnknownEventTypeApply() throws {
        var harness = try seeded()
        let event = try StoreWire.event(
            "agent.somethingNew", effects: [("agents", "upsert", "dev-1", "{\"state\":\"blocked\"}")])
        harness.apply(.event(projectId: "proj-a", event))
        XCTAssertEqual(harness.alpha?.agents["dev-1"]?.state, "blocked")
    }

    func testCursorLineSetsTheHostCursor() throws {
        var harness = try seeded()
        let cursor = makeCursor(["proj-a"])
        harness.apply(try StoreWire.cursor(cursor))
        XCTAssertEqual(harness.host.cursor, cursor)
    }

    func testLatestReadWins() throws {
        var harness = try seeded()
        harness.apply(
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1", state: "lost")),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(harness.alpha?.agents["dev-1"]?.state, "lost")
    }

    func testUpsertKeepsTheStoredFlagsOfTextItDoesNotState() throws {
        var harness = try seeded()
        harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "op:7", item: StoreWire.waitingItem("op:7")),
            try StoreWire.end("proj-a"))
        harness.host.projects["proj-a"]?.waiting["op:7"]?.summary =
            UntrustedText(raw: "cut", verified: false, truncated: true)
        let event = try StoreWire.event(
            "operator.proposed", effects: [("waiting", "upsert", "op:7", "{\"needsTerminal\":true}")])
        harness.apply(.event(projectId: "proj-a", event))
        let item = try XCTUnwrap(harness.alpha?.waiting["op:7"])
        XCTAssertTrue(item.needsTerminal)
        XCTAssertFalse(item.summary.verified, "an unverified summary stays unverified")
        XCTAssertTrue(item.summary.truncated)
        XCTAssertEqual(item.summary.raw, "")
    }

    func testAKeyNotifiedByAnUpsertDuringASnapshotIsNotNotifiedAtItsEnd() throws {
        var harness = ReducerHarness()
        harness.apply(try StoreWire.hello(), try StoreWire.project("proj-a"), try StoreWire.end("proj-a"))
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "op:9", item: StoreWire.waitingItem("op:9")))
        let fields = "{\"requestId\":\"req\",\"kind\":\"op\",\"since\":\"\(StoreWire.since)\",\"summary\":\"go\","
            + "\"contentHash\":\"h\",\"needsTerminal\":false}"
        let event = try StoreWire.event("operator.proposed", effects: [("waiting", "upsert", "op:9", fields)])
        let during = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertEqual(during.notify.map(\.key), ["op:9"])
        let atEnd = harness.apply(try StoreWire.end("proj-a"))
        XCTAssertEqual(atEnd.notify, [])
    }

    func testDisplayNameIsTheLastPathComponent() {
        XCTAssertEqual(ProjectModel(projectId: "p", root: "/srv/work/alpha/").displayName, "alpha")
        XCTAssertEqual(ProjectModel(projectId: "p", root: "/").displayName, "p")
    }
}
