import XCTest
@testable import CapstanKit

final class FixtureTests: XCTestCase {
    func testEveryFixtureDecodes() throws {
        let fixtures = try KernelFixtures.events()
        XCTAssertFalse(fixtures.isEmpty)
        for fixture in fixtures {
            let event = try JSONDecoder().decode(FeedEvent.self, from: fixture.data)
            XCTAssertTrue(fixture.name.hasPrefix(event.type), fixture.name)
            XCTAssertTrue(FeedEventType.known.contains(event.type), fixture.name)
        }
    }

    func testCorruptBodyMessage() throws {
        let body = try XCTUnwrap(try KernelFixtures.event("message.action_needed.corrupt_body").body.messageBody)
        XCTAssertFalse(body.body.verified)
        XCTAssertEqual(body.body.raw, "")
    }

    func testCorruptCommand() throws {
        let body = try XCTUnwrap(try KernelFixtures.event("operator.proposed.corrupt_command").body.operatorBody)
        XCTAssertFalse(body.command.verified)
        XCTAssertEqual(body.command.raw, "")
        XCTAssertFalse(body.reason.verified)
        XCTAssertEqual(body.reason.raw, "")
    }

    func testCorruptPromptText() throws {
        let body = try XCTUnwrap(try KernelFixtures.event("prompt.waiting.corrupt_text").body.promptBody)
        XCTAssertFalse(body.text.verified)
        XCTAssertEqual(body.text.raw, "")
        XCTAssertTrue(body.options.isEmpty)
    }

    func testCorruptPromptOptions() throws {
        let body = try XCTUnwrap(try KernelFixtures.event("prompt.waiting.corrupt_options").body.promptBody)
        XCTAssertFalse(body.text.verified)
        XCTAssertEqual(body.text.raw, "")
        XCTAssertTrue(body.options.isEmpty)
    }

    func testCleanedAndTruncated() throws {
        let body = try XCTUnwrap(try KernelFixtures.event("prompt.waiting.cleaned_and_truncated").body.promptBody)
        XCTAssertTrue(body.text.truncated)
        XCTAssertTrue(body.text.verified)
        XCTAssertFalse(body.text.raw.isEmpty)
    }

    func testPromptWaiting() throws {
        let event = try KernelFixtures.event("prompt.waiting")
        let body = try XCTUnwrap(event.body.promptBody)
        XCTAssertEqual(body.text.raw, "Allow Bash(ls)?")
        XCTAssertEqual(body.options.count, 2)
        XCTAssertEqual(body.options[0].number, 1)
        XCTAssertEqual(body.options[0].text.raw, "Yes")
        XCTAssertFalse(body.options[0].acceptsText)
        XCTAssertTrue(body.options[1].acceptsText)
        XCTAssertEqual(event.body.effects.first?.op, .invalidate)
        XCTAssertNil(event.body.messageBody)
    }

    func testOtherTypedViews() throws {
        let run = try XCTUnwrap(try KernelFixtures.event("run.changed").body.runBody)
        XCTAssertTrue(run.paused)
        XCTAssertEqual(run.reason.raw, "maintenance")
        let design = try XCTUnwrap(try KernelFixtures.event("design.decided").body.designBody)
        XCTAssertEqual(design.decision, "approved")
        XCTAssertEqual(design.link.displayPlain, "https://example.test/ui")
        let blocked = try XCTUnwrap(try KernelFixtures.event("agent.blocked").body.agentBody)
        XCTAssertEqual(blocked.blocker, "permission_prompt")
        XCTAssertNil(blocked.generation)
        XCTAssertNil(blocked.paused)
        XCTAssertNil(blocked.reason)
        let activity = try XCTUnwrap(try KernelFixtures.event("agent.activity").body.activityBody)
        XCTAssertEqual(activity.agentId, "dev-1")
        XCTAssertEqual(activity.activity, "working")
        XCTAssertEqual(activity.activitySince, "2026-01-01T00:00:00.033Z")
        XCTAssertEqual(activity.activitySeq, 1)
        XCTAssertNil(try KernelFixtures.event("agent.activity").body.agentBody)
        let lost = try XCTUnwrap(try KernelFixtures.event("agent.lost").body.agentBody)
        XCTAssertEqual(lost.reason?.raw, "pane_gone")
    }

    func testASignedOffPlanItemDecodes() throws {
        let text = "{\"key\":\"1\",\"planId\":\"plan-1\",\"sequence\":1,\"title\":\"Ledger\",\"state\":\"signed_off\","
            + "\"packages\":{\"total\":3,\"assigned\":3,\"reported\":3,\"integrated\":3}}"
        let plan = try PlanItem(json: try decodeJSON(Data(text.utf8)))
        XCTAssertEqual(plan.state, "signed_off")
        XCTAssertEqual(plan.packages.integrated, 3)
        XCTAssertEqual(StoredPlan(plan).state, "signed_off")
    }

    func testAnAgentItemDecodesWithAndWithoutABranch() throws {
        let plain = StoreWire.agentItem("dev-1")
        let without = try AgentItem(json: try decodeJSON(Data(plain.utf8)))
        XCTAssertNil(without.branch)
        let tail = "\"contentHash\":\"agent-hash\"}"
        let branched = plain.replacingOccurrences(of: tail, with: "\"contentHash\":\"agent-hash\",\"branch\":\"feat/x\"}")
        let with = try AgentItem(json: try decodeJSON(Data(branched.utf8)))
        XCTAssertEqual(with.branch?.raw, "feat/x")
        XCTAssertEqual(StoredAgent(with).branch?.raw, "feat/x")
        XCTAssertNil(StoredAgent(without).branch)
    }
}
