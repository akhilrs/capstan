import CapstanKit
import Foundation
import XCTest
@testable import CapstanAppCore

final class OverviewTests: XCTestCase {
    private let epoch = "daeca3b2245f3519"
    private var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? TimeZone.current
        return calendar
    }

    private func date(_ text: String) throws -> Date {
        try XCTUnwrap(ISO8601DateFormatter().date(from: text))
    }

    private func now() throws -> Date { try date("2026-10-30T14:10:00Z") }

    // MARK: Builders

    private func text(_ raw: String) -> UntrustedText {
        UntrustedText(raw: raw, verified: true, truncated: false)
    }

    private func agent(
        _ id: String, state: String = "active", paused: Bool = false, task: String? = nil, activity: String? = nil,
        since: String? = nil
    ) -> StoredAgent {
        StoredAgent(
            key: id, agentId: id, kind: "Developer", role: "developer", generation: 1, state: state, paused: paused,
            task: task.map { (raw: String) -> UntrustedText in text(raw) }, requestId: "agent:" + id,
            contentHash: "h", branch: nil, activity: activity, activitySince: since,
            activitySeq: activity == nil ? nil : 1)
    }

    private func plan(
        _ key: String, state: String = "approved", title: String = "Ledger", total: Int64 = 6, assigned: Int64 = 4,
        reported: Int64 = 2, integrated: Int64 = 1
    ) -> StoredPlan {
        let counts = StoredCounts(total: total, assigned: assigned, reported: reported, integrated: integrated)
        return StoredPlan(
            key: key, planId: "plan-" + key, sequence: Int64(key) ?? 0, title: text(title), state: state,
            packages: counts)
    }

    private func project(
        agents: [StoredAgent] = [], plans: [StoredPlan] = [], paused: Bool = false, stalled: [String] = []
    ) -> ProjectModel {
        var agentMap: [String: StoredAgent] = [:]
        for item in agents { agentMap[item.key] = item }
        var planMap: [String: StoredPlan] = [:]
        for item in plans { planMap[item.key] = item }
        var attention: [String: String] = [:]
        for id in stalled { attention[id] = "stalled" }
        let run = StoredRun(requestId: "run", state: "active", paused: paused, contentHash: "run-hash")
        return ProjectModel(
            projectId: "p1", root: "/work/p1", epoch: epoch, head: 26, agents: agentMap, plans: planMap, run: run,
            attention: attention, snapshotComplete: true)
    }

    private func current(at text: String = "2026-10-30T14:05:00Z") throws -> ProjectFreshness {
        ProjectFreshness(currentAsOf: try date(text), lastKnownAt: try date(text))
    }

    private func make(
        _ project: ProjectModel, freshness: ProjectFreshness? = nil, history: RecentHistoryState? = nil,
        folded: Set<String> = []
    ) throws -> OverviewModel {
        let fresh = try freshness ?? current()
        let held = history ?? RecentHistoryState(projectId: "p1")
        return OverviewModel.make(
            project: project, freshness: fresh, history: held, folded: folded, now: try now(), calendar: calendar)
    }

    // MARK: Status words

    func testUnknownWhenRefreshingFailedOrNotReady() throws {
        let busy = project(agents: [agent("a", activity: "working")])
        let refreshing = ProjectFreshness(isRefreshing: true)
        XCTAssertEqual(try make(busy, freshness: refreshing).statusWord, "Unknown")
        let failed = ProjectFreshness(currentAsOf: try date("2026-10-30T14:05:00Z"), refreshFailedAt: try date("2026-10-30T14:06:00Z"))
        XCTAssertEqual(try make(busy, freshness: failed).statusWord, "Unknown")
        let silent = ProjectFreshness(availability: .silent)
        XCTAssertEqual(try make(busy, freshness: silent).statusWord, "Unknown")
    }

    func testPausedBeatsEveryAgentState() throws {
        let lost = project(agents: [agent("a", state: "lost")], paused: true)
        XCTAssertEqual(try make(lost).statusWord, "Paused")
    }

    func testNeedsAttentionForLostBlockedOrStalled() throws {
        XCTAssertEqual(try make(project(agents: [agent("a", state: "lost")])).statusWord, "Needs attention")
        XCTAssertEqual(try make(project(agents: [agent("a", state: "blocked")])).statusWord, "Needs attention")
        let stalled = project(agents: [agent("a", activity: "working")], stalled: ["a"])
        XCTAssertEqual(try make(stalled).statusWord, "Needs attention")
        let mixed = project(agents: [agent("a", activity: "working"), agent("b", state: "lost")])
        XCTAssertEqual(try make(mixed).statusWord, "Needs attention")
    }

    func testMovingForAWorkingAgent() throws {
        let busy = project(agents: [agent("a", activity: "working"), agent("b", activity: "idle")])
        XCTAssertEqual(try make(busy).statusWord, "Moving")
    }

    func testNeedsYouAloneIsNotMoving() throws {
        let waiting = project(agents: [agent("a", activity: "blocked"), agent("b", activity: "idle")])
        let model = try make(waiting)
        XCTAssertEqual(model.statusWord, "Quiet")
        XCTAssertEqual(model.countsText, "1 needs you · 1 idle")
    }

    func testQuietWhenNothingElseHolds() throws {
        XCTAssertEqual(try make(project()).statusWord, "Quiet")
        XCTAssertEqual(try make(project(agents: [agent("a", activity: "idle")])).statusWord, "Quiet")
    }

    func testTheQuietFactLinesComeFromCountsOnly() throws {
        XCTAssertEqual(try make(project()).quietFact, "No plan in progress")
        let merged = plan("1", state: "merged", total: 3, assigned: 3, reported: 3, integrated: 3)
        let other = plan("2", state: "merged", total: 2, assigned: 2, reported: 2, integrated: 2)
        XCTAssertEqual(try make(project(plans: [merged, other])).quietFact, "No plan in progress · 2 plans integrated")
        let open = project(agents: [agent("a", activity: "idle")], plans: [plan("3")])
        XCTAssertEqual(try make(open).quietFact, "No active agent has a task")
        XCTAssertNil(try make(project(agents: [agent("a", activity: "working")])).quietFact)
    }

    // MARK: Counts

    func testCountsOmitZerosAndFollowTheHealthOrder() throws {
        let agents = [
            agent("a", activity: "working"), agent("b", activity: "working"), agent("c", activity: "idle"),
            agent("d", activity: "idle"), agent("e", state: "lost"),
        ]
        let model = try make(project(agents: agents))
        XCTAssertEqual(model.countsText, "2 working · 1 lost · 2 idle")
        XCTAssertEqual(model.counts.count, 3)
        XCTAssertTrue(model.countsKnown)
    }

    func testCountsShowUnknownWhileRefreshing() throws {
        let busy = project(agents: [agent("a", activity: "working")])
        let model = try make(busy, freshness: ProjectFreshness(isRefreshing: true))
        XCTAssertFalse(model.countsKnown)
        XCTAssertEqual(model.countsText, "–")
        XCTAssertTrue(model.counts.isEmpty)
        XCTAssertEqual(model.freshnessLine, "Refreshing…")
        XCTAssertTrue(model.isLastKnown)
    }

    func testTheStripReadsAsOneLine() throws {
        let agents = [
            agent("a", activity: "working"), agent("b", activity: "working"), agent("c", activity: "working"),
            agent("d", activity: "idle"), agent("e", activity: "idle"),
        ]
        let model = try make(project(agents: agents))
        XCTAssertEqual(model.statusLine, "Moving · 3 working · 2 idle · current as of 14:05")
        XCTAssertEqual(model.accessibilityValue, "Moving: 3 working, 2 idle. Current as of 14:05.")
    }

    // MARK: Freshness lines

    func testTheFreshnessLines() throws {
        let busy = project()
        let pending = ProjectFreshness(
            pendingKeys: ["agents/a"], currentAsOf: try date("2026-10-30T14:05:00Z"),
            lastKnownAt: try date("2026-10-30T14:05:00Z"))
        XCTAssertEqual(try make(busy, freshness: pending).freshnessLine, "current as of 14:05 · 1 item updating")
        let failed = ProjectFreshness(
            lastKnownAt: try date("2026-10-30T14:02:00Z"), refreshFailedAt: try date("2026-10-30T14:06:00Z"))
        XCTAssertEqual(
            try make(busy, freshness: failed).freshnessLine, "Refresh failed at 14:06 · showing last known from 14:02")
        let silent = ProjectFreshness(liveness: .silent, lastKnownAt: try date("2026-10-30T14:02:00Z"))
        let silentModel = try make(busy, freshness: silent)
        XCTAssertEqual(silentModel.freshnessLine, "last known, 14:02")
        XCTAssertTrue(silentModel.isLastKnown)
    }

    func testAPendingRowIsMarkedForDimming() throws {
        let busy = project(agents: [agent("a", activity: "working")], plans: [plan("1")])
        let pending = ProjectFreshness(pendingKeys: ["agents/a"], currentAsOf: try date("2026-10-30T14:05:00Z"))
        let model = try make(busy, freshness: pending)
        XCTAssertTrue(model.agentsNow.first?.isPending ?? false)
        XCTAssertFalse(model.plans.first?.isPending ?? true)
    }

    // MARK: Agents now

    func testAgentsNowCapsAndOrdersExceptionsThenWorkingByOldestSince() throws {
        var agents: [StoredAgent] = [
            agent("l1", state: "lost"), agent("l2", state: "lost"), agent("b1", state: "blocked"),
            agent("s1", activity: "working"), agent("s2", activity: "working"), agent("n1", activity: "blocked"),
        ]
        agents.append(agent("w1", activity: "working", since: "2026-10-30T14:00:00Z"))
        agents.append(agent("w2", activity: "working", since: "2026-10-30T13:20:00Z"))
        agents.append(agent("w3", activity: "working", since: "2026-10-30T14:08:00Z"))
        agents.append(agent("w4", activity: "working", since: "2026-10-30T12:00:00Z"))
        agents.append(agent("w5", activity: "working"))
        let model = try make(project(agents: agents, stalled: ["s1", "s2"]))
        let ids = model.agentsNow.map { (row: OverviewAgent) -> String in row.agentId }
        XCTAssertEqual(ids, ["l1", "l2", "b1", "s1", "w4", "w2", "w1"])
        XCTAssertEqual(model.moreNeedAttention, 2)
        XCTAssertEqual(model.moreNeedAttentionText, "+2 more need attention")
        XCTAssertEqual(model.totalAgents, 11)
        XCTAssertEqual(model.viewAllText, "View all 11 agents")
    }

    func testAWorkingAgentShowsHowLongAndNothingUnderClockSkew() throws {
        let agents = [
            agent("a", activity: "working", since: "2026-10-30T14:04:00Z"),
            agent("b", activity: "working", since: "2026-10-30T15:00:00Z"),
            agent("c", activity: "working", since: "garbage"),
        ]
        let model = try make(project(agents: agents))
        func age(_ id: String) -> String? {
            model.agentsNow.first { (row: OverviewAgent) -> Bool in row.agentId == id }?.workingFor
        }
        XCTAssertEqual(age("a"), "6m")
        XCTAssertNil(age("b"))
        XCTAssertNil(age("c"))
    }

    func testTheEmptyTexts() throws {
        let model = try make(project())
        XCTAssertEqual(model.agentsEmptyText, "No active agent")
        XCTAssertEqual(model.plansEmptyText, "No plan in progress")
        XCTAssertNil(model.viewAllText)
        XCTAssertNil(model.moreNeedAttentionText)
    }

    // MARK: Plans

    func testPlanFactLinesComeFromCountsAndState() throws {
        let plans = [
            plan("1"), plan("2", state: "signed_off"), plan("3", state: "submitted"), plan("4", state: "merged"),
            plan("5", state: "approved", total: 0, assigned: 0, reported: 0, integrated: 0),
        ]
        let model = try make(project(plans: plans))
        let lines = model.plans.map { (row: OverviewPlan) -> String in row.factLine }
        XCTAssertEqual(
            lines,
            [
                "2 unassigned · 2 assigned · 1 reported, not integrated · 1 of 6 integrated",
                "signed off, not merged", "submitted for approval", "approved, no packages",
            ])
        XCTAssertEqual(model.plans.first?.progressLabel, "1 of 6 integrated")
    }

    // MARK: Outcomes

    private func event(seq: Int, ts: String, type: String = "report.recorded") throws -> FeedEvent {
        let head = "{\"id\":\"p1:\(epoch):\(seq)\",\"seq\":\(seq),\"k\":0,\"ts\":\"\(ts)\",\"type\":\"\(type)\","
        let middle = "\"actorId\":\"dev-1\",\"entity\":{\"type\":\"agent_report\",\"id\":\"r\(seq)\"},\"from\":null,\"to\":null,"
        let body = "\"body\":{\"effects\":[],\"agentId\":\"dev-4\",\"commitSha\":\"a03f9c1d5b7e2f4a6c8e0b1d3f5a7c9e1b3d5f70\"}}"
        let value = try JSONDecoder().decode(JSONValue.self, from: Data([head, middle, body].joined().utf8))
        return try FeedEvent(json: value)
    }

    private func history(_ events: [FeedEvent]) throws -> RecentHistoryState {
        var held = RecentHistoryState(projectId: "p1")
        held.begin(epoch: epoch, head: 26)
        let page = RecentPage(projectId: "p1", epoch: epoch, head: 26, events: events, truncated: false)
        held.apply(.page(page), at: try now())
        return held
    }

    func testOutcomeTimesAreHostTimesWithTheReceiptTimeApart() throws {
        let events = [
            try event(seq: 1, ts: "2026-10-29T18:40:00.000Z"), try event(seq: 2, ts: "2026-10-30T13:55:00.000Z"),
            try event(seq: 3, ts: "2026-10-30T14:09:30.000Z"), try event(seq: 4, ts: "2026-10-30T14:30:00.000Z"),
            try event(seq: 5, ts: "2026-10-20T09:05:00.000Z"),
        ]
        let model = try make(project(), history: try history(events))
        func row(_ seq: Int64) throws -> OverviewOutcome {
            try XCTUnwrap(model.outcomes.first { (outcome: OverviewOutcome) -> Bool in outcome.seq == seq })
        }
        XCTAssertEqual(try row(1).hostTime, "yesterday 18:40")
        XCTAssertEqual(try row(2).hostTime, "13:55")
        XCTAssertEqual(try row(3).hostTime, "now")
        XCTAssertEqual(try row(4).hostTime, "14:30")
        XCTAssertTrue(try row(4).isSkewed)
        XCTAssertFalse(try row(1).isSkewed)
        XCTAssertEqual(try row(5).hostTime, "20 Oct 09:05")
        XCTAssertEqual(try row(1).receivedTime, "14:10")
        XCTAssertEqual(model.outcomes.map { (outcome: OverviewOutcome) -> Int64 in outcome.seq }, [5, 4, 3, 2, 1])
        XCTAssertEqual(try row(2).text, "dev-4 reported a03f9c1")
    }

    func testHistoryStatesAndTheirTexts() throws {
        let busy = project()
        let t = try date("2026-10-30T14:02:00Z")
        var loading = RecentHistoryState(projectId: "p1")
        loading.begin(epoch: epoch, head: 26)
        XCTAssertEqual(try make(busy, history: loading).historyState, .loading)

        var session = RecentHistoryState(projectId: "p1")
        session.connected(at: t)
        session.apply(.sessionOnly, at: t)
        let sessionModel = try make(busy, history: session)
        XCTAssertEqual(sessionModel.historyState, .sessionOnly)
        XCTAssertEqual(
            sessionModel.historyText,
            "This host can't show earlier history. Showing what happened since you connected at 14:02.")
        XCTAssertEqual(sessionModel.outcomesEmptyText, "No outcomes since you connected at 14:02")

        var failing = RecentHistoryState(projectId: "p1")
        failing.begin(epoch: epoch, head: 26)
        failing.apply(.error(code: "x", message: "y"), at: t)
        let errorModel = try make(busy, history: failing)
        XCTAssertEqual(errorModel.historyState, .error)
        XCTAssertEqual(errorModel.historyText, "Couldn't load earlier history")
        XCTAssertTrue(errorModel.canRetryHistory)

        var limited = RecentHistoryState(projectId: "p1")
        limited.begin(epoch: epoch, head: 26)
        limited.apply(.rateLimited, at: try now())
        XCTAssertFalse(try make(busy, history: limited).canRetryHistory)
    }

    func testTruncatedAndGapStates() throws {
        var truncated = RecentHistoryState(projectId: "p1")
        truncated.begin(epoch: epoch, head: 26)
        let page = RecentPage(projectId: "p1", epoch: epoch, head: 26, events: [], truncated: true)
        truncated.apply(.page(page), at: try now())
        let model = try make(project(), history: truncated)
        XCTAssertEqual(model.historyState, .truncated)
        XCTAssertEqual(model.outcomesEmptyText, "No outcomes in the last 500 events")
        XCTAssertEqual(model.historyText, "Earlier history not loaded: the host stopped after its scan limit")

        var gap = RecentHistoryState(projectId: "p1")
        gap.resync(at: try date("2026-10-30T14:06:00Z"))
        XCTAssertEqual(try make(project(), history: gap).historyGapText, "History before 14:06 may be missing")
    }

    // MARK: Untrusted text

    func testAgentAndPlanTextNeverReachTheStatusCountsOrFactLines() throws {
        let hostile = "\u{202E}\u{07}Needs attention"
        let withText = project(
            agents: [agent("a", task: hostile, activity: "working")], plans: [plan("1", title: hostile)])
        let plain = project(agents: [agent("a", task: "x", activity: "working")], plans: [plan("1", title: "y")])
        let model = try make(withText)
        let control = try make(plain)
        XCTAssertEqual(model.statusWord, "Moving")
        XCTAssertEqual(model.statusWord, control.statusWord)
        XCTAssertEqual(model.countsText, control.countsText)
        XCTAssertEqual(model.statusLine, control.statusLine)
        XCTAssertEqual(model.accessibilityValue, control.accessibilityValue)
        XCTAssertEqual(model.plans.map { (row: OverviewPlan) -> String in row.factLine },
                       control.plans.map { (row: OverviewPlan) -> String in row.factLine })
        XCTAssertFalse(model.statusLine.contains("Needs attention"))

        let task = try XCTUnwrap(model.agentsNow.first?.task)
        XCTAssertTrue(task.hasPrefix("\u{2068}"))
        XCTAssertTrue(task.hasSuffix("\u{2069}"))
        XCTAssertTrue(task.contains("Needs attention"))
        XCTAssertFalse(task.contains("\u{202E}"))
        XCTAssertFalse(task.contains("\u{07}"))
        let title = try XCTUnwrap(model.plans.first?.title)
        XCTAssertTrue(title.hasPrefix("\u{2068}"))
        XCTAssertFalse(title.contains("\u{202E}"))
        XCTAssertFalse(title.contains("\u{07}"))
    }

    // MARK: List column and folding

    func testTheListColumnShowsTheOverviewForASelectedProjectAndTheEmptyStateOnlyAcrossProjects() {
        XCTAssertEqual(OverviewModel.listColumn(selectedProjectId: "p1", anythingWaiting: false), .overview)
        XCTAssertEqual(OverviewModel.listColumn(selectedProjectId: "p1", anythingWaiting: true), .overview)
        XCTAssertEqual(OverviewModel.listColumn(selectedProjectId: nil, anythingWaiting: false), .emptyState)
        XCTAssertEqual(OverviewModel.listColumn(selectedProjectId: nil, anythingWaiting: true), .waitingGroups)
    }

    func testFoldedSectionsComeFromTheProjectAndAreKeptPerProject() throws {
        var folded = project()
        folded.foldedSections = ["plans"]
        let model = try make(folded, folded: folded.folded)
        XCTAssertTrue(model.isFolded(OverviewModel.plansFolded))
        XCTAssertFalse(model.isFolded(OverviewModel.agentsFolded))
        XCTAssertTrue(model.showsOverview)
        let other = try make(project(), folded: project().folded)
        XCTAssertFalse(other.isFolded(OverviewModel.plansFolded))
    }
}
