import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class GroupsTests: XCTestCase {
    private let hostA = ShellFixtures.hostA
    private let hostB = ShellFixtures.hostB

    // MARK: Builders

    private func agent(_ id: String, role: String = "developer") throws -> StoredAgent {
        try ShellFixtures.decode(
            StoredAgent.self,
            "{\"key\":\"agent:\(id)\",\"agentId\":\"\(id)\",\"kind\":\"claude\",\"role\":\"\(role)\",\"generation\":1,"
                + "\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:\(id)\",\"contentHash\":\"h\"}")
    }

    private func row(_ id: String, _ health: AgentHealth, role: String = "developer") throws -> AgentRow {
        AgentRow(agent: try agent(id, role: role), health: health)
    }

    private func group(_ name: String, host: String = "Host", id: UUID? = nil, _ rows: [AgentRow]) -> AgentGroup {
        AgentGroup(hostId: id ?? hostA, projectId: name.lowercased(), projectName: name, hostName: host, rows: rows)
    }

    private func plan(
        _ key: String, state: String = "approved", total: Int64 = 4, assigned: Int64 = 2, reported: Int64 = 1,
        integrated: Int64 = 0
    ) -> StoredPlan {
        StoredPlan(
            key: key, planId: "plan-" + key, sequence: Int64(key) ?? 0,
            title: UntrustedText(raw: "Plan " + key, verified: true, truncated: false), state: state,
            packages: StoredCounts(total: total, assigned: assigned, reported: reported, integrated: integrated))
    }

    private func planGroup(_ name: String, host: String = "Host", _ plans: [StoredPlan]) -> PlanGroup {
        var map: [String: StoredPlan] = [:]
        for item in plans { map[item.key] = item }
        let project = ProjectModel(projectId: name.lowercased(), root: "/work/" + name.lowercased(), plans: map)
        return PlanGroup(
            hostId: hostA, projectId: project.projectId, projectName: name, hostName: host,
            rows: PlanRow.rows(project), project: project)
    }

    private func items(_ group: PlanGroup) -> [PlanItem] {
        group.rows.map { PlanItem(group: group, row: $0, waiting: 0) }
    }

    // MARK: Agents

    func testAttentionListsNeedsYouBeforeTheRest() throws {
        let one = group("One", [try row("a1", .working), try row("a2", .lost), try row("a3", .idle)])
        let two = group("Two", [try row("b1", .needsYou), try row("b2", .blocked)])
        let list = AgentShow.attention([one, two])
        XCTAssertEqual(list.first?.row.agentId, "b1", "needs you first")
        XCTAssertEqual(Set(list.map { $0.row.agentId }), ["b1", "a2", "b2"])
        XCTAssertEqual(list.first?.id, hostA.uuidString + "/two/agent:b1")
        XCTAssertEqual(list.first?.group.projectName, "Two")
    }

    func testMatchingFiltersAndSearches() throws {
        let g = group("Docs", host: "Build", [
            try row("dev-1", .working), try row("dev-2", .needsYou), try row("rev-1", .working, role: "reviewer"),
            try row("dev-3", .paused),
        ])
        let all = AgentShow.matching(g, show: .all, needle: "").map(\.agentId)
        XCTAssertEqual([all.first, all.last], ["dev-2", "dev-3"], "needs you first, paused after working")
        XCTAssertEqual(Set(all), ["dev-1", "dev-2", "dev-3", "rev-1"])
        XCTAssertEqual(AgentShow.matching(g, show: .attention, needle: "").map(\.agentId), ["dev-2"])
        XCTAssertEqual(Set(AgentShow.matching(g, show: .working, needle: "").map(\.agentId)), ["dev-1", "rev-1"])
        XCTAssertEqual(AgentShow.matching(g, show: .paused, needle: "").map(\.agentId), ["dev-3"])
        XCTAssertEqual(AgentShow.matching(g, show: .all, needle: "REVIEWER").map(\.agentId), ["rev-1"], "role, any case")
        XCTAssertEqual(AgentShow.matching(g, show: .all, needle: "dev-1").map(\.agentId), ["dev-1"])
        XCTAssertEqual(AgentShow.matching(g, show: .working, needle: "docs").count, 2, "a project name keeps its agents")
        XCTAssertEqual(AgentShow.matching(g, show: .all, needle: "build").count, 4, "a host name keeps its agents")
        XCTAssertTrue(AgentShow.matching(g, show: .all, needle: "zzz").isEmpty)
    }

    func testGroupsAreOrderedByNameThenHost() {
        let groups = [group("beta", host: "B", []), group("Alpha", host: "Z", []), group("alpha 2", host: "A", []),
                      group("Alpha", host: "A", [])]
        let sorted = groups.sorted(by: AgentShow.byName)
        XCTAssertEqual(sorted.map { $0.projectName + "/" + $0.hostName }, ["Alpha/A", "Alpha/Z", "alpha 2/A", "beta/B"])
    }

    func testAgentSummaryAndCounts() throws {
        XCTAssertEqual(AgentShow.summary(1, projects: 1), "1 agent · 1 project")
        XCTAssertEqual(AgentShow.summary(7, projects: 2), "7 agents · 2 projects")
        let counts = HealthCounts([
            try row("a", .needsYou), try row("b", .lost), try row("c", .working), try row("d", .idle), try row("e", .idle),
        ])
        XCTAssertEqual(counts.alert, "1 needs you · 1 lost")
        XCTAssertEqual(counts.rest, "1 working · 2 idle")
        XCTAssertEqual(counts.tone, .accent)
        XCTAssertEqual(counts.headline, "1 needs you")
        XCTAssertEqual(counts.barCounts, [1, 1, 1, 0, 2])
        let idle = HealthCounts([try row("a", .idle)])
        XCTAssertEqual(idle.rest, "all idle")
        XCTAssertEqual(idle.headline, "All idle")
        XCTAssertEqual(idle.tone, .grey)
        XCTAssertEqual(HealthCounts([try row("a", .paused)]).headline, "All paused")
    }

    func testAgentGroupsOfReadyProjectsWithAgents() throws {
        let busy = try ShellFixtures.project("zeta", agents: [try ShellFixtures.agent(id: "a1", requestId: "r", hash: "h")])
        let empty = try ShellFixtures.project("empty")
        let beta = try ShellFixtures.project("alpha", agents: [try ShellFixtures.agent(id: "b1", requestId: "r", hash: "h")])
        let hosts = [
            HostConfig(id: hostA, displayName: "Mac", destination: "mac"),
            HostConfig(id: hostB, displayName: "Build", destination: "build"),
        ]
        let groups = AgentGroup.make(
            hosts: hosts, hostModels: [hostA: ShellFixtures.host(hostA, projects: [busy, empty]),
                                       hostB: ShellFixtures.host(hostB, projects: [beta])])
        XCTAssertEqual(groups.map { $0.hostName + "/" + $0.projectName }, ["Build/alpha", "Mac/zeta"])
        XCTAssertEqual(groups[0].jumpEntry.count, "1 agent")
        XCTAssertEqual(groups[0].id, hostB.uuidString + "/alpha")
    }

    // MARK: Pins and the jump sheet

    func testPinsToggleAndRecentsKeepFiveNewestFirst() {
        XCTAssertEqual(ProjectPins.list(""), [])
        XCTAssertEqual(ProjectPins.list("a\nb"), ["a", "b"])
        var raw = ProjectPins.toggled("a", in: "")
        raw = ProjectPins.toggled("b", in: raw)
        XCTAssertEqual(raw, "a\nb")
        XCTAssertEqual(ProjectPins.toggled("a", in: raw), "b")
        var recent = ""
        for id in ["1", "2", "3", "4", "5", "6"] { recent = ProjectPins.touched(id, in: recent) }
        XCTAssertEqual(ProjectPins.list(recent), ["6", "5", "4", "3", "2"])
        XCTAssertEqual(ProjectPins.list(ProjectPins.touched("3", in: recent)), ["3", "6", "5", "4", "2"])
        XCTAssertEqual(ProjectPins.recentCap, 5)
    }

    // MARK: Plans

    func testPinnedProjectsComeFirstThenAToZ() {
        let groups = [planGroup("Gamma", [plan("1")]), planGroup("alpha", [plan("1")]), planGroup("Beta", [plan("1")])]
        let pinned = [groups[0].id]
        XCTAssertEqual(PlanShow.ordered(groups, pinned: pinned).map(\.projectName), ["Gamma", "alpha", "Beta"])
        XCTAssertEqual(PlanShow.ordered(groups, pinned: []).map(\.projectName), ["alpha", "Beta", "Gamma"])
        XCTAssertEqual(PlanShow.ordered(groups, pinned: ["gone", groups[2].id]).map(\.projectName), ["Beta", "alpha", "Gamma"])
    }

    func testPlanShowSplitsPlansByState() {
        let rows = PlanRow.rows(ProjectModel(
            projectId: "p", root: "/w/p",
            plans: ["1": plan("1"), "2": plan("2", state: "in_review"), "3": plan("3", state: "signed_off"),
                    "4": plan("4", total: 2, assigned: 2, reported: 2, integrated: 2)]))
        let byId = Dictionary(uniqueKeysWithValues: rows.map { ($0.planId, PlanShow.of($0)) })
        XCTAssertEqual(byId["plan-1"], .active)
        XCTAssertEqual(byId["plan-2"], .review)
        XCTAssertEqual(byId["plan-3"], .done)
        XCTAssertEqual(byId["plan-4"], .done)
    }

    func testSectionsForAllProjectsAreOnePerProject() {
        let a = planGroup("Alpha", [plan("1"), plan("2")])
        let b = planGroup("Beta", [plan("3")])
        let empty = planGroup("Empty", [plan("4")])
        let shown = items(a) + items(b)
        let sections = PlanShow.sections(shown, groups: [a, b, empty], scope: nil, show: .active)
        XCTAssertEqual(sections.map(\.title), ["Alpha", "Beta"])
        XCTAssertEqual(sections[0].items.count, 2)
        XCTAssertEqual(sections[0].subtitle, "Host")
        XCTAssertFalse(sections[0].openByDefault)
        XCTAssertEqual(sections[0].total, 8)
    }

    func testSectionsForOneProjectSplitActiveByState() {
        let g = planGroup("Alpha", [plan("1"), plan("2", state: "draft"), plan("3")])
        let all = items(g)
        let waiting = all.map { PlanItem(group: $0.group, row: $0.row, waiting: $0.row.planId == "plan-3" ? 1 : 0) }
        let sections = PlanShow.sections(waiting, groups: [g], scope: g.id, show: .active)
        XCTAssertEqual(sections.map(\.id), ["waits", "going", "drafts"])
        XCTAssertEqual(sections[0].items.map { $0.row.planId }, ["plan-3"])
        XCTAssertEqual(sections[1].items.map { $0.row.planId }, ["plan-1"])
        XCTAssertEqual(sections[2].items.map { $0.row.planId }, ["plan-2"])
        XCTAssertEqual([sections[0].openByDefault, sections[1].openByDefault, sections[2].openByDefault], [true, true, false])
        XCTAssertEqual(sections[0].tone, .accent)
        XCTAssertEqual(PlanShow.sections(all, groups: [g], scope: g.id, show: .review).map(\.title), ["Waiting for review"])
        XCTAssertEqual(PlanShow.sections(all, groups: [g], scope: g.id, show: .done).map(\.title), ["Finished"])
        XCTAssertTrue(PlanShow.sections([], groups: [g], scope: g.id, show: .done).isEmpty)
    }

    func testTitlesSummariesAndJumpEntries() {
        XCTAssertEqual(PlanShow.groupTitle(scope: nil, show: .done), "Done · by project")
        XCTAssertEqual(PlanShow.groupTitle(scope: "x", show: .active), "Active · by state")
        XCTAssertEqual(PlanShow.groupTitle(scope: "x", show: .review), "In review")
        XCTAssertEqual(PlanShow.summary(1, projects: 1, scope: nil, all: true), "1 plan · 1 project")
        XCTAssertEqual(PlanShow.summary(34, projects: 3, scope: nil, all: true), "34 plans · 3 projects")
        XCTAssertEqual(PlanShow.summary(2, projects: 1, scope: "Alpha", all: false), "2 plans in Alpha")
        let entry = planGroup("Alpha", [plan("1"), plan("2", state: "signed_off")]).jumpEntry
        XCTAssertEqual(entry.count, "2 plans")
        XCTAssertEqual(entry.state, "1 active")
        XCTAssertEqual(entry.tone, .green)
        let none = planGroup("Done", [plan("1", state: "signed_off")]).jumpEntry
        XCTAssertEqual(none.state, "None active")
        XCTAssertEqual(none.tone, .grey)
    }

    // MARK: Lists for many projects (plan-149)

    func testAttentionCountAndFilterCountsFollowTheSearch() throws {
        let one = group("One", host: "Build", [try row("a1", .working), try row("a2", .lost), try row("a3", .paused)])
        let two = group("Two", host: "Mac", [try row("b1", .needsYou), try row("rev-1", .idle, role: "reviewer")])
        XCTAssertEqual(AgentShow.attentionCount([one, two]), 2)
        XCTAssertEqual(AgentShow.attentionCount([]), 0)
        let all = AgentShow.counts([one, two], needle: "")
        XCTAssertEqual([all[.all], all[.attention], all[.working], all[.paused]], [5, 2, 1, 1])
        let searched = AgentShow.counts([one, two], needle: "reviewer")
        XCTAssertEqual([searched[.all], searched[.attention], searched[.working], searched[.paused]], [1, 0, 0, 0])
        let byProject = AgentShow.counts([one, two], needle: "one")
        XCTAssertEqual(byProject[.all], 3, "a project name keeps all its agents")
    }

    func testEmptinessHasThreeCauses() {
        XCTAssertEqual(AgentShow.emptiness(all: 0, shown: 0, show: .all, needle: ""), .noAgents)
        XCTAssertEqual(AgentShow.emptiness(all: 9, shown: 0, show: .paused, needle: ""), .nothingForFilter(.paused))
        XCTAssertEqual(AgentShow.emptiness(all: 9, shown: 0, show: .all, needle: "qa-bot"), .noMatch("qa-bot"))
        XCTAssertNil(AgentShow.emptiness(all: 9, shown: 2, show: .all, needle: "x"))
        XCTAssertEqual(AgentEmptiness.noMatch("qa-bot").title, "No results for \"qa-bot\"")
        XCTAssertEqual(AgentEmptiness.nothingForFilter(.paused).detail, "No agent is paused.")
        XCTAssertEqual(AgentEmptiness.noAgents.title, "No agents")
    }

    func testPlanNeedsYouSearchAndCounts() {
        let g = planGroup("Alpha", [plan("1"), plan("2", state: "in_review"), plan("3", state: "signed_off"), plan("4")])
        let rows = g.rows
        let shown = rows.map { PlanItem(group: g, row: $0, waiting: $0.planId == "plan-4" ? 2 : 0) }
        XCTAssertEqual(PlanShow.needsYou(shown).map { $0.row.planId }, ["plan-4"])
        XCTAssertEqual(PlanShow.matching(shown, needle: "PLAN-3").map { $0.row.planId }, ["plan-3"])
        XCTAssertEqual(PlanShow.matching(shown, needle: "").count, 4)
        let counts = PlanShow.counts(shown, needle: "")
        XCTAssertEqual([counts[.active], counts[.review], counts[.done]], [2, 1, 1])
        XCTAssertEqual(PlanShow.counts(shown, needle: "plan 2")[.review], 1)
        XCTAssertNil(PlanShow.counts(shown, needle: "plan 2")[.active])
    }

    func testPinnedQuery() {
        XCTAssertTrue(ProjectPins.isPinned("a", in: "a\nb"))
        XCTAssertFalse(ProjectPins.isPinned("c", in: "a\nb"))
        XCTAssertFalse(ProjectPins.isPinned("a", in: ""))
    }
}
