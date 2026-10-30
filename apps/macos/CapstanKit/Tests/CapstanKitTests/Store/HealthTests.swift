import Foundation
import XCTest
@testable import CapstanKit

final class HealthTests: XCTestCase {
    private func agentJSON(
        agentState: String = "active", paused: Bool = false, task: String? = nil, activity: String? = nil,
        activitySince: String? = nil, role: String = "developer"
    ) -> String {
        let item = StoreWire.agentItem("dev-1", state: agentState, paused: paused, task: task)
            .replacingOccurrences(of: "\"role\":\"developer\"", with: "\"role\":\"\(role)\"")
        var extra = ""
        if let activity { extra += "\"activity\":\"\(activity)\",\"activitySeq\":1," }
        if let activitySince { extra += "\"activitySince\":\"\(activitySince)\"," }
        return item.replacingOccurrences(of: "\"requestId\"", with: extra + "\"requestId\"")
    }

    private func project(
        agentState: String = "active", paused: Bool = false, task: String? = nil, activity: String? = nil,
        activitySince: String? = nil, role: String = "developer"
    ) throws -> ReducerHarness {
        let item = agentJSON(
            agentState: agentState, paused: paused, task: task, activity: activity, activitySince: activitySince,
            role: role)
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "agents", key: "dev-1", item: item),
            try StoreWire.end("proj-a"))
        return harness
    }

    private func basis(_ harness: ReducerHarness) throws -> HealthBasis {
        let alpha = try XCTUnwrap(harness.alpha)
        return AgentHealth.basis(of: try XCTUnwrap(alpha.agents["dev-1"]), in: alpha)
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
            try StoreWire.snapshot(
                "proj-a", "agents", key: "dev-1", item: agentJSON(task: "build", activity: "idle")),
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

    func testActivityIdleIsIdleEvenWithATask() throws {
        let harness = try project(task: "build", activity: "idle")
        XCTAssertEqual(try health(harness), .idle)
        XCTAssertEqual(try basis(harness), .host(.idle, since: nil))
    }

    func testActivityWorkingIsWorkingForReviewersAndArchitectsWithoutATask() throws {
        for role in ["reviewer", "architect", "developer"] {
            let harness = try project(task: nil, activity: "working", role: role)
            XCTAssertEqual(try health(harness), .working, role)
            XCTAssertEqual(try basis(harness), .host(.working, since: nil))
        }
    }

    func testActivityBlockedNeedsYou() throws {
        let harness = try project(task: "build", activity: "blocked")
        XCTAssertEqual(try health(harness), .needsYou)
        XCTAssertEqual(try basis(harness), .host(.blocked, since: nil))
        XCTAssertEqual(AgentHealth.needsYou.word, "Needs you")
    }

    func testActivityDoneIsIdle() throws {
        let harness = try project(task: "build", activity: "done")
        XCTAssertEqual(try health(harness), .idle)
        XCTAssertEqual(try basis(harness), .host(.done, since: nil))
    }

    func testActivityUnknownUsesTheTaskRule() throws {
        XCTAssertEqual(try health(try project(task: "build", activity: "unknown")), .working)
        XCTAssertEqual(try health(try project(task: nil, activity: "unknown")), .idle)
        XCTAssertEqual(try basis(try project(task: "build", activity: "unknown")), .estimate(.unknown))
    }

    func testAnotherActivityStringUsesTheTaskRule() throws {
        XCTAssertEqual(try health(try project(task: "build", activity: "napping")), .working)
        XCTAssertEqual(try health(try project(agentState: "idle", task: "build", activity: "napping")), .idle)
        XCTAssertEqual(try basis(try project(task: "build", activity: "napping")), .estimate(.unrecognised))
    }

    func testAbsentActivityKeepsTodaysTaskRule() throws {
        XCTAssertEqual(try health(try project(task: "build")), .working)
        XCTAssertEqual(try health(try project(task: nil)), .idle)
        XCTAssertEqual(try health(try project(agentState: "idle", task: "build")), .idle)
        XCTAssertEqual(try basis(try project(task: "build")), .estimate(.absent))
    }

    func testPausedLostBlockedStateAndStalledOutrankActivity() throws {
        XCTAssertEqual(try health(try project(paused: true, task: "t", activity: "working")), .paused)
        XCTAssertEqual(try health(try project(agentState: "paused", activity: "working")), .paused)
        XCTAssertEqual(try health(try project(agentState: "lost", activity: "working")), .lost)
        XCTAssertEqual(try health(try project(agentState: "blocked", activity: "idle")), .blocked)
        var stalled = try project(task: "build", activity: "working")
        stalled.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        XCTAssertEqual(try health(stalled), .stalled)
        XCTAssertEqual(try basis(stalled), .state)
        XCTAssertNil(try basis(stalled).line(now: Date()))
    }

    func testAWaitingPromptStillNeedsYouWhateverTheActivity() throws {
        var harness = try project(task: "build", activity: "idle")
        harness.apply(
            .event(projectId: "proj-a", try KernelFixtures.event("prompt.waiting")),
            try StoreWire.snapshot("proj-a", "waiting", key: "prompt:1", item: StoreWire.waitingItem("prompt:1")),
            try StoreWire.snapshot(
                "proj-a", "agents", key: "dev-1", item: agentJSON(task: "build", activity: "idle")),
            try StoreWire.end("proj-a", head: 10))
        XCTAssertEqual(try health(harness), .needsYou)
        XCTAssertEqual(try basis(harness), .state)
    }

    func testAnActivityEventDoesNotClearAttention() throws {
        var harness = try project(task: "build")
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("plan.needs_attention")))
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("agent.activity")))
        XCTAssertEqual(harness.alpha?.attention["dev-1"], "stalled")
    }

    func testTheBasisLine() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        func at(_ seconds: TimeInterval) -> String? {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return formatter.string(from: now.addingTimeInterval(-seconds))
        }
        let en = Locale(identifier: "en_GB")
        func line(_ since: String?) -> String? { HealthBasis.host(.working, since: since).line(now: now, locale: en) }
        XCTAssertEqual(line(at(30)), "Host activity: working, for less than a minute.")
        XCTAssertEqual(line(at(6 * 60)), "Host activity: working, for 6 min.")
        XCTAssertEqual(line(at(125 * 60)), "Host activity: working, for 2 h 5 min.")
        XCTAssertEqual(line(at(3 * 3600)), "Host activity: working, for 3 h.")
        XCTAssertTrue(try XCTUnwrap(line(at(30 * 3600))).hasPrefix("Host activity: working, since "))
        XCTAssertEqual(line(nil), "Host activity: working.")
        XCTAssertEqual(line("not a time"), "Host activity: working.")
        XCTAssertEqual(line(at(-600)), "Host activity: working.")
        XCTAssertEqual(
            HealthBasis.estimate(.unrecognised).line(now: now), "Estimate: the host reported an activity this app does not know.")
        XCTAssertEqual(HealthBasis.estimate(.absent).line(now: now), "Estimate: the host reported no activity.")
        XCTAssertEqual(HealthBasis.estimate(.unknown).line(now: now), "Estimate: the host reported activity unknown.")
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
