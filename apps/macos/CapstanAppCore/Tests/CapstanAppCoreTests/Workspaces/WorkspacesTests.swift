import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class WorkspacesTests: XCTestCase {
    private func agent(
        id: String = "a1", state: String = "active", paused: Bool = false, task: String? = "Write views"
    ) throws -> StoredAgent {
        let taskPart = task.map { ",\"task\":{\"raw\":\"\($0)\",\"verified\":true,\"truncated\":false}" } ?? ""
        return try ShellFixtures.decode(
            StoredAgent.self,
            "{\"key\":\"agent:\(id)\",\"agentId\":\"\(id)\",\"kind\":\"claude\",\"role\":\"developer\",\"generation\":1,"
                + "\"state\":\"\(state)\",\"paused\":\(paused)\(taskPart),\"requestId\":\"agent:\(id)\",\"contentHash\":\"h\"}")
    }

    private func plan(_ total: Int64, _ assigned: Int64, _ reported: Int64, _ integrated: Int64) -> StoredPlan {
        StoredPlan(
            key: "plan:p", planId: "p", sequence: 1,
            title: UntrustedText(raw: "Plan", verified: true, truncated: false), state: "running",
            packages: StoredCounts(total: total, assigned: assigned, reported: reported, integrated: integrated))
    }

    func testAgentRowWordsAndSymbolsForEveryHealth() throws {
        let words = ["Working", "Needs you", "Stalled", "Lost", "Blocked", "Paused", "Idle"]
        XCTAssertEqual(AgentHealth.allCases.map { AgentRow(agent: try! agent(), health: $0).healthWord }, words)
        for health in AgentHealth.allCases {
            let row = AgentRow(agent: try agent(), health: health)
            XCTAssertEqual(row.healthSymbol, health.symbolName)
            XCTAssertFalse(row.healthSymbol.isEmpty)
        }
    }

    func testPausedAgentShowsPausedWhateverTheHealth() throws {
        let row = AgentRow(agent: try agent(paused: true), health: .working)
        XCTAssertEqual(row.healthWord, "Paused")
        XCTAssertTrue(row.paused)
    }

    func testAgentRowTaskIsOneLineAndUnverifiedIsHidden() throws {
        XCTAssertEqual(AgentRow(agent: try agent(task: "a\\nb"), health: .idle).task, "a b")
        XCTAssertNil(AgentRow(agent: try agent(task: nil), health: .idle).task)
        var hidden = try agent()
        hidden.task = UntrustedText(raw: "secret", verified: false, truncated: false)
        let row = AgentRow(agent: hidden, health: .idle)
        XCTAssertNil(row.task)
        XCTAssertTrue(row.taskUnverified)
    }

    func testProjectWithPausedRunShowsProjectPaused() throws {
        let paused = ProjectPauseControl(decide: true, busy: false, runPaused: true)
        XCTAssertEqual(paused.status, "Project paused")
        XCTAssertEqual(paused.action, .resume)
        XCTAssertNil(ProjectPauseControl(decide: true, busy: false, runPaused: false).status)
    }

    func testPlanRowWords() {
        let none = PlanRow(plan(4, 0, 0, 0))
        XCTAssertEqual(none.words, "0 of 4 integrated, 4 not started")
        let mid = PlanRow(plan(4, 3, 2, 1))
        XCTAssertEqual(mid.words, "1 of 4 integrated, 1 reported, 1 assigned, 1 not started")
        XCTAssertEqual(PlanRow(plan(4, 4, 4, 4)).words, "4 of 4 integrated")
        XCTAssertEqual(PlanRow(plan(0, 0, 0, 0)).words, "No packages")
    }

    func testPlanRowClampsInconsistentCounts() {
        let row = PlanRow(plan(4, 1, 3, 9))
        XCTAssertEqual(row.integrated, 4)
        XCTAssertEqual(row.notStarted, 0)
        XCTAssertEqual(row.words, "4 of 4 integrated")
    }

    func testPlanLookupFindsTheStoredPlanOfARow() {
        var project = ProjectModel(projectId: "a", root: "/a", snapshotComplete: true)
        let first = plan(4, 0, 0, 0)
        project.plans[first.key] = first
        let row = PlanRow(first)
        XCTAssertEqual(PlanRow.plan(for: row, in: project)?.key, first.key)
        let dirty = StoredPlan(
            key: "plan:q", planId: "q\nx", sequence: 2, title: first.title, state: "approved", packages: first.packages)
        project.plans[dirty.key] = dirty
        XCTAssertEqual(PlanRow.plan(for: PlanRow(dirty), in: project)?.key, dirty.key)
        XCTAssertEqual(PlanRow.plan(for: row, in: project)?.key, first.key)
        let empty = ProjectModel(projectId: "b", root: "/b", snapshotComplete: true)
        XCTAssertNil(PlanRow.plan(for: row, in: empty))
    }

    func testProjectPauseControl() {
        XCTAssertEqual(ProjectPauseControl(decide: true, busy: false, runPaused: false).action, .pause)
        XCTAssertEqual(ProjectPauseControl(decide: true, busy: false, runPaused: false).state, .enabled)
        XCTAssertEqual(ProjectPauseControl(decide: true, busy: false, runPaused: true).action, .resume)
        XCTAssertEqual(
            ProjectPauseControl(decide: false, busy: false, runPaused: false).state,
            .disabled(PauseReason.noDecide))
        XCTAssertEqual(
            ProjectPauseControl(decide: true, busy: true, runPaused: true).state, .disabled(PauseReason.sending))
        let key = BusyKey(hostId: ShellFixtures.hostA, projectId: "p1", requestId: "run")
        let caps = HostCapabilities(reachable: true, events: true, decide: true)
        XCTAssertEqual(
            ProjectPauseControl(capabilities: caps, busy: [key], key: key, runPaused: false).state,
            .disabled("Sending"))
        XCTAssertEqual(ProjectPauseControl(capabilities: nil, busy: [], key: key, runPaused: false).state,
            .disabled(PauseReason.noDecide))
    }

    func testPauseControl() {
        XCTAssertEqual(PauseControl(decide: true, busy: false, paused: false).pause, .enabled)
        XCTAssertEqual(PauseControl(decide: true, busy: false, paused: false).resume, .disabled("Not paused"))
        XCTAssertEqual(PauseControl(decide: true, busy: false, paused: true).resume, .enabled)
        XCTAssertEqual(PauseControl(decide: true, busy: false, paused: true).pause, .disabled("Already paused"))
        let none = PauseControl(decide: false, busy: false, paused: true)
        XCTAssertEqual(none.pause, .disabled(PauseReason.noDecide))
        XCTAssertEqual(none.resume, .disabled(PauseReason.noDecide))
        let busy = PauseControl(decide: true, busy: true, paused: false)
        XCTAssertEqual(busy.pause, .disabled("Sending"))
        XCTAssertEqual(busy.resume, .disabled("Sending"))
    }

    func testPauseControlFromBusyKeys() {
        let key = BusyKey(hostId: ShellFixtures.hostA, projectId: "p1", requestId: "agent:a1")
        let caps = HostCapabilities(reachable: true, events: true, decide: true)
        XCTAssertEqual(PauseControl(capabilities: caps, busy: [key], key: key, paused: false).pause, .disabled("Sending"))
        XCTAssertEqual(PauseControl(capabilities: caps, busy: [], key: key, paused: false).pause, .enabled)
    }

    func testAgentFilterCounts() throws {
        let rows = [
            AgentRow(agent: try agent(id: "a"), health: .working),
            AgentRow(agent: try agent(id: "b"), health: .stalled),
            AgentRow(agent: try agent(id: "c"), health: .needsYou),
            AgentRow(agent: try agent(id: "d", paused: true), health: .working),
        ]
        XCTAssertEqual(AgentFilter.all.count(rows), 4)
        XCTAssertEqual(AgentFilter.needAttention.count(rows), 2)
        XCTAssertEqual(AgentFilter.paused.count(rows), 1)
    }

    private func plan(state: String, _ total: Int64 = 3, _ integrated: Int64 = 0) -> PlanRow {
        var stored = plan(total, integrated, integrated, integrated)
        stored.state = state
        return PlanRow(stored)
    }

    func testPlanFilterDoneIsEndedOrAllIntegratedAndActiveIsTheRest() {
        let rows = [
            plan(state: "approved"), plan(state: "draft"), plan(state: "in_review"), plan(state: "signed_off"),
            plan(state: "approved", 3, 3), plan(state: "superseded", 0, 0), plan(state: "cancelled", 0, 0),
            plan(state: "rejected", 0, 0), plan(state: "failed", 0, 0),
        ]
        XCTAssertEqual(rows.map { PlanFilter.filter(of: $0) }, [
            .active, .active, .active, .done, .done, .done, .done, .done, .done,
        ])
        XCTAssertEqual(PlanFilter.counts(rows).active, 2)
        XCTAssertEqual(PlanFilter.counts(rows).done, 6)
        XCTAssertFalse(PlanFilter.active.matches(rows[1]))
        XCTAssertFalse(PlanFilter.done.matches(rows[1]))
        XCTAssertEqual(rows[1].metaLine, "draft · 3 packages")
        XCTAssertEqual(rows[1].pill(waiting: 0), PlanPill(word: "Draft", tone: .grey))
    }

    func testDraftsAreGroupedPerProjectAndLeftOutOfTheActiveList() {
        let first = [plan(state: "approved"), plan(state: "draft"), plan(state: "draft"), plan(state: "signed_off")]
        let second = [plan(state: "draft"), plan(state: "in_review")]
        XCTAssertEqual(PlanFilter.drafts(in: first).count, 2)
        XCTAssertEqual(PlanFilter.drafts(in: second).count, 1)
        XCTAssertEqual(first.filter(PlanFilter.active.matches).map(\.state), ["approved"])
        XCTAssertEqual(second.filter(PlanFilter.active.matches).map(\.state), ["in_review"])
        XCTAssertTrue(PlanFilter.drafts(in: first).allSatisfy { $0.pill(waiting: 0) == PlanPill(word: "Draft", tone: .grey) })
        XCTAssertEqual(PlanFilter.draftsWord(1), "1 draft")
        XCTAssertEqual(PlanFilter.draftsWord(2), "2 drafts")
        XCTAssertEqual(PlanFilter.counts(first + second).active, 2)
    }

    private func integrationItem(_ key: String, since: String) throws -> StoredWaiting {
        try ShellFixtures.decode(
            StoredWaiting.self,
            "{\"key\":\"\(key)\",\"requestId\":\"r\",\"kind\":\"integration\",\"since\":\"\(since)\","
                + "\"summary\":{\"raw\":\"s\",\"verified\":true,\"truncated\":false},\"contentHash\":\"h\",\"needsTerminal\":false}")
    }

    private func detailed(_ key: String, integrations: [String?]) -> StoredPlan {
        var stored = plan(2, 2, 2, 0)
        stored.key = key
        stored.planId = key
        stored.packageDetails = integrations.enumerated().map { index, id in
            PackageDetail(
                id: "p\(index)", title: UntrustedText(raw: "t", verified: true, truncated: false), role: "developer",
                state: .reported, integrationId: id)
        }
        return stored
    }

    func testWaitingItemsAreMatchedToThePlanByIntegrationIdAndOrderedOldestFirst() throws {
        var project = ProjectModel(projectId: "a", root: "/a", snapshotComplete: true)
        let mine = detailed("plan-1", integrations: ["7", "3"])
        let other = detailed("plan-2", integrations: ["9"])
        for item in [
            try integrationItem("integration:7", since: "2026-01-02"), try integrationItem("integration:3", since: "2026-01-01"),
            try integrationItem("integration:9", since: "2026-01-01"),
        ] { project.waiting[item.key] = item }
        XCTAssertEqual(IntegrationLink.waiting(for: mine, in: project).map(\.key), ["integration:3", "integration:7"])
        XCTAssertEqual(IntegrationLink.waiting(for: other, in: project).map(\.key), ["integration:9"])
        XCTAssertEqual(IntegrationLink.waiting(for: detailed("plan-3", integrations: [nil]), in: project), [])
        XCTAssertEqual(IntegrationLink.waiting(for: nil, in: project), [])
        var bare = plan(2, 2, 2, 0)
        bare.packageDetails = nil
        XCTAssertEqual(IntegrationLink.waiting(for: bare, in: project), [])
    }

    func testPlanMetaLineAndPill() {
        XCTAssertEqual(plan(state: "approved", 5, 0).metaLine, "approved · 5 packages")
        var stored = plan(5, 4, 3, 2)
        stored.state = "approved"
        let mid = PlanRow(stored)
        XCTAssertEqual(mid.metaLine, "approved · 5 packages · 2 integrated, 1 reported, 1 assigned")
        XCTAssertEqual(mid.pill(waiting: 1), PlanPill(word: "1 waits for you", tone: .yellow))
        XCTAssertNil(mid.pill(waiting: 0))
        XCTAssertEqual(mid.summaryLine, "2 of 5 integrated")
        let signed = plan(state: "signed_off", 3, 3)
        XCTAssertEqual(signed.metaLine, "signed off · 3 of 3 integrated")
        XCTAssertEqual(signed.pill(waiting: 0), PlanPill(word: "Ready to merge", tone: .green))
        XCTAssertEqual(plan(state: "in_review").pill(waiting: 0), PlanPill(word: "In review", tone: .grey))
        XCTAssertNil(plan(state: "approved").pill(waiting: 2))
    }

    private func feedModel(_ lines: [String]) throws -> ProjectModel {
        var host = HostModel(hostId: ShellFixtures.hostA)
        var staging = Staging()
        for text in lines {
            let line = try LineDecoder().decode(Data(text.utf8))
            _ = StoreReducer.apply(line, to: &host, staging: &staging)
        }
        return try XCTUnwrap(host.projects["p1"])
    }

    private static let feedStart = [
        "{\"v\":1,\"kind\":\"hello\",\"version\":\"0.1.0\",\"host\":\"h\",\"projects\":1}",
        "{\"v\":1,\"kind\":\"project\",\"projectId\":\"p1\",\"root\":\"/srv/p1\",\"state\":\"ready\","
            + "\"epoch\":\"daeca3b2245f3519\",\"head\":26}",
        "{\"v\":1,\"kind\":\"snapshot\",\"projectId\":\"p1\",\"section\":\"run\",\"key\":\"run\","
            + "\"item\":{\"requestId\":\"run\",\"state\":\"active\",\"paused\":false,\"contentHash\":\"h1\"}}",
        "{\"v\":1,\"kind\":\"snapshotEnd\",\"projectId\":\"p1\",\"head\":26}",
    ]

    private static func runChanged(_ state: String, _ paused: Bool, seq: Int) -> String {
        let head = "{\"v\":1,\"kind\":\"event\",\"projectId\":\"p1\",\"event\":{"
        let ident = "\"id\":\"p1:e:\(seq)\",\"seq\":\(seq),\"k\":0,"
        let meta = "\"ts\":\"2026-10-30T01:07:06.000Z\",\"type\":\"run.changed\",\"actorId\":null,"
        let entity = "\"entity\":{\"type\":\"run_control\",\"id\":\"p1\"},\"from\":null,\"to\":null,"
        let effects = "\"body\":{\"effects\":[{\"section\":\"run\",\"op\":\"invalidate\",\"key\":\"run\",\"fields\":{}}],"
        let fact = "\"state\":\"\(state)\",\"paused\":\(paused),"
        let tail = "\"reason\":\"Test\",\"textVerified\":true,\"truncated\":false,\"untrusted\":[\"body.reason\"]}}}"
        return [head, ident, meta, entity, effects, fact, tail].joined()
    }

    func testPauseArrivingThroughTheFeedTurnsTheControlToResume() throws {
        let before = try feedModel(Self.feedStart)
        XCTAssertEqual(ProjectPauseControl(decide: true, busy: false, runPaused: before.run?.paused ?? false).action, .pause)
        let after = try feedModel(Self.feedStart + [Self.runChanged("paused", true, seq: 27)] as [String])
        let paused = after.run?.paused ?? false
        XCTAssertTrue(paused)
        let control = ProjectPauseControl(decide: true, busy: false, runPaused: paused)
        XCTAssertEqual(control.action, .resume)
        XCTAssertEqual(control.status, ProjectPauseControl.pausedWord)
        // The event drops the stale hash: no request target until the re-read brings the new one.
        XCTAssertNil(RequestTarget.forRun(after))
        var readLines: [String] = Self.feedStart
        readLines.append(Self.runChanged("paused", true, seq: 27))
        readLines.append(contentsOf: Self.runSnapshot(state: "paused", paused: true, hash: "h2"))
        let read = try feedModel(readLines)
        XCTAssertEqual(RequestTarget.forRun(read)?.requestId, "run")
    }

    func testResumeArrivingThroughTheFeedTurnsTheControlBackToPause() throws {
        let model = try feedModel(
            Self.feedStart + [Self.runChanged("paused", true, seq: 27), Self.runChanged("active", false, seq: 28)] as [String])
        let paused = model.run?.paused ?? true
        XCTAssertFalse(paused)
        XCTAssertEqual(ProjectPauseControl(decide: true, busy: false, runPaused: paused).action, .pause)
    }

    private static func runSnapshot(state: String, paused: Bool, hash: String) -> [String] {
        let project = "{\"v\":1,\"kind\":\"project\",\"projectId\":\"p1\",\"root\":\"/srv/p1\","
            + "\"state\":\"ready\",\"epoch\":\"daeca3b2245f3519\",\"head\":27}"
        let itemHead = "{\"v\":1,\"kind\":\"snapshot\",\"projectId\":\"p1\",\"section\":\"run\",\"key\":\"run\","
        let item = "\"item\":{\"requestId\":\"run\",\"state\":\"\(state)\",\"paused\":\(paused),\"contentHash\":\"\(hash)\"}}"
        let end = "{\"v\":1,\"kind\":\"snapshotEnd\",\"projectId\":\"p1\",\"head\":27}"
        return [project, [itemHead, item].joined(), end]
    }

    func testResumeAfterPauseThroughTheFeedCarriesTheNewHashAndNeverTheOldOne() throws {
        let afterEvent = try feedModel(Self.feedStart + [Self.runChanged("paused", true, seq: 27)] as [String])
        // Until the read, the old hash "h1" is not offered: no request is built and the control waits.
        XCTAssertNil(RequestTarget.forRun(afterEvent))
        let waiting = ProjectPauseControl(
            decide: true, busy: false, runPaused: true, hashKnown: afterEvent.run?.hashKnown ?? true)
        XCTAssertEqual(waiting.state, .disabled(PauseReason.updating))

        var readLines: [String] = Self.feedStart
        readLines.append(Self.runChanged("paused", true, seq: 27))
        readLines.append(contentsOf: Self.runSnapshot(state: "paused", paused: true, hash: "h2"))
        let read = try feedModel(readLines)
        XCTAssertEqual(RequestTarget.forRun(read)?.expect, "h2")
        let route = DecideRouter.route(
            .runResume(reason: nil), target: .run, project: read, busy: [], hostId: ShellFixtures.hostA,
            projectId: "p1", client: "mac", nonce: "n-1")
        guard case .send(let request, _) = route else { return XCTFail("a resume with a known hash is sent") }
        XCTAssertEqual(request.expect, "h2")
        XCTAssertTrue(ProjectPauseControl(decide: true, busy: false, runPaused: true, hashKnown: true).state.isEnabled)
    }

    func testRunChangedShowsUpdatingThenResumeAfterTheReread() throws {
        func control(_ model: ProjectModel) -> ProjectPauseControl {
            ProjectPauseControl(
                decide: true, busy: false, runPaused: model.run?.paused ?? false,
                hashKnown: model.run?.hashKnown ?? true)
        }
        let changed = try feedModel(Self.feedStart + [Self.runChanged("paused", true, seq: 27)] as [String])
        XCTAssertEqual(control(changed).action, .resume)
        XCTAssertEqual(control(changed).state, .disabled(PauseReason.updating))
        var readLines: [String] = Self.feedStart
        readLines.append(Self.runChanged("paused", true, seq: 27))
        readLines.append(contentsOf: Self.runSnapshot(state: "paused", paused: true, hash: "h2"))
        let read = try feedModel(readLines)
        XCTAssertEqual(control(read).action, .resume)
        XCTAssertEqual(control(read).state, .enabled)
    }
}
