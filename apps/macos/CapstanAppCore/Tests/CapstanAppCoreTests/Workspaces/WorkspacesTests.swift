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

    func testPlanFilterSplitsDoneFromActive() {
        let rows = [PlanRow(plan(3, 3, 3, 3)), PlanRow(plan(3, 2, 1, 0)), PlanRow(plan(0, 0, 0, 0))]
        XCTAssertEqual(PlanFilter.counts(rows).done, 1)
        XCTAssertEqual(PlanFilter.counts(rows).active, 2)
        XCTAssertTrue(PlanFilter.done.matches(rows[0]))
        XCTAssertTrue(PlanFilter.active.matches(rows[2]))
    }
}
