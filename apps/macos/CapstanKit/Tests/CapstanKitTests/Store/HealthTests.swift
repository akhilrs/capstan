import Foundation
import XCTest
@testable import CapstanKit

final class HealthTests: XCTestCase {
    private func project(agentState: String = "active", paused: Bool = false, task: String? = nil) throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot(
                "proj-a", "agents", key: "dev-1",
                item: StoreWire.agentItem("dev-1", state: agentState, paused: paused, task: task)),
            try StoreWire.end("proj-a"))
        return harness
    }

    private func health(_ harness: ReducerHarness) throws -> AgentHealth {
        let alpha = try XCTUnwrap(harness.alpha)
        return alpha.health(of: try XCTUnwrap(alpha.agents["dev-1"]))
    }

    private func body(_ event: FeedEvent, _ key: String) -> JSONValue? {
        event.body.raw[key]
    }

    func testAttentionFromTheFixtureMakesTheAgentStalled() throws {
        var harness = try project(task: "build")
        XCTAssertEqual(try health(harness), .working)
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        XCTAssertEqual(harness.alpha?.attention["dev-1"], "stalled")
        XCTAssertEqual(try health(harness), .stalled)
    }

    func testALaterAgentEventClearsAttention() throws {
        var harness = try project()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("agent.resumed")))
        XCTAssertNil(harness.alpha?.attention["dev-1"])
        XCTAssertEqual(try health(harness), .idle)
    }

    func testAResyncAndASnapshotClearAttention() throws {
        var harness = try project()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        harness.apply(try StoreWire.resync("proj-a"))
        XCTAssertTrue(try XCTUnwrap(harness.alpha).attention.isEmpty)
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        harness.apply(try StoreWire.end("proj-a"))
        XCTAssertTrue(try XCTUnwrap(harness.alpha).attention.isEmpty)
    }

    func testLostFixture() throws {
        var harness = try project()
        let event = try KernelFixtures.event("agent.lost")
        let output = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertEqual(output.refresh, ["proj-a"])
        guard case .string(let state)? = body(event, "state") else { return XCTFail("fixture has no state") }
        harness.apply(
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1", state: state)),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(try health(harness), .lost)
    }

    func testBlockedFixture() throws {
        var harness = try project()
        let event = try KernelFixtures.event("agent.blocked")
        XCTAssertEqual(harness.apply(.event(projectId: "proj-a", event)).refresh, ["proj-a"])
        guard case .string(let state)? = body(event, "state") else { return XCTFail("fixture has no state") }
        harness.apply(
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1", state: state)),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(try health(harness), .blocked)
    }

    func testPausedFixture() throws {
        var harness = try project(task: "build")
        let event = try KernelFixtures.event("agent.paused")
        XCTAssertEqual(harness.apply(.event(projectId: "proj-a", event)).refresh, ["proj-a"])
        guard case .bool(let paused)? = body(event, "paused") else { return XCTFail("fixture has no paused") }
        harness.apply(
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1", paused: paused)),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(try health(harness), .paused)
    }

    func testAWaitingPromptThatNamesTheAgentMakesItNeedYou() throws {
        var harness = try project(task: "build")
        harness.apply(
            .event(projectId: "proj-a", try KernelFixtures.event("prompt.waiting")),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1", task: "build")),
            try StoreWire.snapshot("proj-a", "waiting", key: "prompt:1", item: StoreWire.waitingItem("prompt:1")),
            try StoreWire.end("proj-a", head: 10))
        XCTAssertEqual(try health(harness), .needsYou)
    }

    func testPromptDetailHoldsTheOptionsSeenLive() throws {
        var harness = try project()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("prompt.waiting")))
        let detail = try XCTUnwrap(harness.alpha?.details["prompt:1"])
        XCTAssertEqual(detail.promptBody?.options.count, 2)
        XCTAssertEqual(detail.promptBody?.text.raw, "Allow Bash(ls)?")
        XCTAssertTrue(detail.textVerified)
    }

    func testCorruptOptionsGiveVerifiedFalse() throws {
        var harness = try project()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("prompt.waiting.corrupt_options")))
        let detail = try XCTUnwrap(harness.alpha?.details["prompt:1"])
        XCTAssertFalse(detail.textVerified)
        XCTAssertEqual(detail.promptBody?.text.verified, false)
        XCTAssertEqual(detail.promptBody?.options.count, 0)
    }

    func testTheOperatorCommandIsKeptInFull() throws {
        var harness = try project()
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("operator.proposed")))
        XCTAssertEqual(harness.alpha?.details["op:1"]?.operatorBody?.command.raw, "ls -la")
    }

    func testEachHealthHasAWordAndASymbol() {
        for health in AgentHealth.allCases {
            XCTAssertFalse(health.word.isEmpty)
            XCTAssertFalse(health.symbolName.isEmpty)
        }
    }

    func testPlanProgressFractions() {
        let plan = StoredPlan(
            key: "1", planId: "p", sequence: 1, title: UntrustedText(raw: "t", verified: true, truncated: false),
            state: "approved", packages: StoredCounts(total: 4, assigned: 3, reported: 2, integrated: 1))
        let progress = PlanProgress(plan)
        XCTAssertEqual(progress.integrated, 0.25)
        XCTAssertEqual(progress.reported, 0.25)
        XCTAssertEqual(progress.assigned, 0.25)
        XCTAssertEqual(progress.notStarted, 0.25)
        let empty = StoredPlan(
            key: "2", planId: "q", sequence: 2, title: plan.title, state: "draft",
            packages: StoredCounts(total: 0, assigned: 0, reported: 0, integrated: 0))
        XCTAssertEqual(PlanProgress(empty).notStarted, 0)
    }
}
