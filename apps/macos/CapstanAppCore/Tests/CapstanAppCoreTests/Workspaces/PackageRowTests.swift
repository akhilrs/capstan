import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class PackageRowTests: XCTestCase {
    private func text(_ raw: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: false)
    }

    private func detail(
        _ id: String, state: PackageState, title: UntrustedText? = nil, review: String? = nil,
        dependsOn: [String] = [], assignee: String? = nil, integration: String? = nil
    ) -> PackageDetail {
        PackageDetail(
            id: id, title: title ?? text("Title \(id)"), role: "developer", assigneeAgentId: assignee, state: state,
            dependsOn: dependsOn, review: review, integrationId: nil, integrationState: integration)
    }

    private func plan(_ details: [PackageDetail]?, omitted: Int64? = nil) -> StoredPlan {
        StoredPlan(
            key: "plan:p", planId: "p", sequence: 1, title: text("Plan"), state: "approved",
            packages: StoredCounts(total: 4, assigned: 0, reported: 0, integrated: 0), packageDetails: details,
            packageDetailsOmitted: omitted)
    }

    func testOrderIsTheHostsAndCancelledStayLast() {
        let rows = PackageRow.rows(
            plan([detail("b", state: .assigned), detail("a", state: .unassigned), detail("z", state: .cancelled)]))
        XCTAssertEqual(rows.map(\.id), ["b", "a", "z"])
        XCTAssertEqual(rows.last?.stateWord, "Cancelled")
    }

    func testStateAndReviewWords() {
        let rows = PackageRow.rows(
            plan([
                detail("a", state: .reported, review: "passed", dependsOn: ["x"], assignee: "dev-1"),
                detail("b", state: .integrated, review: "findings", integration: "merged"),
                detail("c", state: .other("later")),
            ]))
        XCTAssertEqual(rows[0].stateWord, "Reported")
        XCTAssertEqual(rows[0].reviewWord, "Passed")
        XCTAssertEqual(rows[0].dependsOn, ["x"])
        XCTAssertEqual(rows[0].assignee, "dev-1")
        XCTAssertNil(rows[0].integrationWord)
        XCTAssertEqual(rows[1].reviewWord, "Changes requested")
        XCTAssertEqual(rows[1].integrationWord, "Merged")
        XCTAssertEqual(rows[2].reviewWord, "Not reviewed")
        XCTAssertEqual(rows[2].stateWord, "later")
    }

    func testUnverifiedTitleShowsThePackageId() {
        let rows = PackageRow.rows(plan([detail("pkg-1", state: .assigned, title: text("secret", verified: false))]))
        XCTAssertEqual(rows[0].title, "pkg-1")
    }

    func testOmittedCountAndMissingDetails() {
        XCTAssertEqual(PlanRow(plan([], omitted: 3)).omittedPackages, 3)
        XCTAssertEqual(PlanRow(plan(nil)).omittedPackages, 0)
        XCTAssertEqual(PackageRow.rows(plan(nil)), [])
    }

    func testPillIconAndOrderLines() {
        let details = [
            detail("retry-core", state: .integrated, review: "passed"),
            detail("timeouts", state: .integrated, review: "passed", dependsOn: ["retry-core"]),
            detail("client-errors", state: .reported, review: "findings", dependsOn: ["timeouts"]),
            detail("docs", state: .unassigned, dependsOn: ["client-errors", "fix-PM-412"]),
            detail("fix-PM-412", state: .assigned, dependsOn: ["timeouts"]),
        ]
        let rows = PackageRow.rows(plan(details))
        XCTAssertEqual(rows.map(\.pillWord), ["Integrated", "Integrated", "Reported", "Not started", "Assigned"])
        XCTAssertEqual(rows.map(\.statusIcon), ["checkmark", "checkmark", "clock", "circle.dashed", "circle"])
        XCTAssertEqual(rows[0].detailLine, "review passed · no dependencies")
        XCTAssertEqual(rows[1].detailLine, "review passed · after retry-core")
        XCTAssertEqual(rows[2].detailLine, "review: changes requested · after timeouts")
        XCTAssertEqual(rows[3].detailLine, "no branch yet · blocked by client-errors and fix-PM-412")
        XCTAssertEqual(rows[4].detailLine, "not reviewed · after timeouts")
        XCTAssertEqual(rows[3].pillTone, .grey)
        XCTAssertEqual(rows[2].pillTone, .blue)
    }

    func testBlockedByListsThreeNamesWithCommas() {
        let details = [
            detail("a", state: .assigned), detail("b", state: .assigned), detail("c", state: .reported),
            detail("d", state: .unassigned, dependsOn: ["a", "b", "c"]),
        ]
        XCTAssertEqual(PackageRow.rows(plan(details))[3].detailLine, "no branch yet · blocked by a, b and c")
    }

    func testRowsWithAnAgentShowItsBranchAndAPromptThatWaits() throws {
        let agentJson = "{\"key\":\"agent:dev-1\",\"agentId\":\"dev-1\",\"kind\":\"claude\",\"role\":\"developer\","
            + "\"generation\":1,\"state\":\"active\",\"paused\":false,"
            + "\"branch\":{\"raw\":\"fix/PM-412-retry\",\"verified\":true,\"truncated\":false},"
            + "\"requestId\":\"agent:dev-1\",\"contentHash\":\"h\"}"
        let agent = try JSONDecoder().decode(StoredAgent.self, from: Data(agentJson.utf8))
        var project = ProjectModel(projectId: "a", root: "/a", snapshotComplete: true)
        project.agents[agent.key] = agent
        let rows = PackageRow.rows(
            plan([detail("fix-PM-412", state: .assigned, assignee: "dev-1"), detail("other", state: .assigned, assignee: "dev-2")]),
            in: project)
        XCTAssertEqual(rows[0].branch, "fix/PM-412-retry")
        XCTAssertEqual(rows[0].pillWord, "Assigned")
        XCTAssertEqual(rows[0].detailLine, "not reviewed · no dependencies")
        XCTAssertNil(rows[1].branch)

        let waiting = StoredWaiting(
            key: "prompt:1", requestId: "r", kind: .prompt, since: "2026-01-01",
            summary: text("Allow?"), contentHash: "h", needsTerminal: false)
        project.waiting[waiting.key] = waiting
        project.details[waiting.key] = ItemDetail(
            key: waiting.key, eventType: "prompt.waiting", seq: 1,
            body: .object([
                "relayId": .string("r1"), "agentId": .string("dev-1"), "state": .string("waiting"),
                "text": .string("Allow?"), "options": .array([]), "effects": .array([]),
            ]))
        let waits = PackageRow.rows(plan([detail("fix-PM-412", state: .assigned, assignee: "dev-1")]), in: project)
        XCTAssertEqual(waits[0].pillWord, "Waits for you")
        XCTAssertEqual(waits[0].pillTone, .yellow)
        XCTAssertEqual(waits[0].detailLine, "prompt open · not reviewed · no dependencies")
    }

    func testAnotherAgentsPromptDoesNotLightAPackage() throws {
        func stored(_ id: String) throws -> StoredAgent {
            let json = "{\"key\":\"agent:\(id)\",\"agentId\":\"\(id)\",\"kind\":\"claude\",\"role\":\"developer\","
                + "\"generation\":1,\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:\(id)\",\"contentHash\":\"h\"}"
            return try JSONDecoder().decode(StoredAgent.self, from: Data(json.utf8))
        }
        var project = ProjectModel(projectId: "a", root: "/a", snapshotComplete: true)
        for id in ["dev-1", "dev-2"] {
            let agent = try stored(id)
            project.agents[agent.key] = agent
        }
        let waiting = StoredWaiting(
            key: "prompt:1", requestId: "r", kind: .prompt, since: "2026-01-01",
            summary: text("Allow?"), contentHash: "h", needsTerminal: false)
        project.waiting[waiting.key] = waiting
        project.details[waiting.key] = ItemDetail(
            key: waiting.key, eventType: "prompt.waiting", seq: 1,
            body: .object([
                "relayId": .string("r1"), "agentId": .string("dev-2"), "state": .string("waiting"),
                "text": .string("Allow?"), "options": .array([]), "effects": .array([]),
            ]))
        let rows = PackageRow.rows(
            plan([detail("one", state: .assigned, assignee: "dev-1"), detail("two", state: .assigned, assignee: "dev-2")]),
            in: project)
        XCTAssertEqual(rows.map(\.pillWord), ["Assigned", "Waits for you"])
        XCTAssertEqual(rows[0].detailLine, "not reviewed · no dependencies")
    }
}
