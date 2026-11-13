import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class WorkspaceOverviewTests: XCTestCase {
    private func member(_ repo: String, _ position: Int64, _ state: String) -> SetMember {
        SetMember(repo: repo, position: position, state: state, headSha: "3f2a9c1d5b7e")
    }

    private func waiting(_ key: String, set: IntegrationSet) -> StoredWaiting {
        StoredWaiting(
            key: key, requestId: key, kind: .integration, since: "s",
            summary: UntrustedText(raw: "merged b", verified: true, truncated: false), contentHash: "h",
            needsTerminal: false, set: set)
    }

    private func agent(_ id: String, trees: [AgentWorktree]?, folder: String?, branch: String? = nil) -> StoredAgent {
        StoredAgent(
            key: id, agentId: id, kind: "Developer", role: "developer", generation: 1, state: "active", paused: false,
            task: nil, requestId: id, contentHash: "h",
            branch: branch.map { UntrustedText(raw: $0, verified: true, truncated: false) }, worktrees: trees,
            changeFolder: folder)
    }

    private func project(waiting: [StoredWaiting] = [], agents: [StoredAgent] = []) throws -> ProjectModel {
        try ShellFixtures.project("p", waiting: waiting, agents: agents)
    }

    func testReposComeInMergeOrderWithTheirOpenSetsAndWorktrees() throws {
        let members = [member("api", 0, "confirmed"), member("web", 1, "merged"), member("worker", 2, "merged")]
        let set = IntegrationSet(setId: "s1", state: "merged", repo: "web", position: 1, members: members)
        let model = try project(
            waiting: [waiting("integration:1", set: set)],
            agents: [agent("dev-4", trees: [AgentWorktree(repo: "web", path: "/w/web", branch: "b")], folder: "/w")])
        let overview = WorkspaceOverview.make(model)
        XCTAssertEqual(overview.repos.map(\.repo), ["api", "web", "worker"])
        XCTAssertEqual(overview.repos.map(\.openSets), [0, 1, 1])
        XCTAssertEqual(overview.repos.map(\.worktrees), [0, 1, 0])
        XCTAssertEqual(overview.repos[1].mergeOrder, "Merge order 2")
        XCTAssertEqual(overview.reposCount, "Repos · 3 in merge order")
        XCTAssertFalse(overview.isEmpty)
    }

    func testTwoItemsOfOneSetCountOnce() throws {
        let members = [member("api", 0, "merged"), member("web", 1, "merged")]
        let one = IntegrationSet(setId: "s1", state: "merged", repo: "api", position: 0, members: members)
        let two = IntegrationSet(setId: "s1", state: "merged", repo: "web", position: 1, members: members)
        let overview = WorkspaceOverview.make(
            try project(waiting: [waiting("integration:1", set: one), waiting("integration:2", set: two)]))
        XCTAssertEqual(overview.repos.map(\.openSets), [1, 1])
    }

    func testARepoOnlyAnAgentHoldsComesAfterThePositionedOnes() throws {
        let overview = WorkspaceOverview.make(
            try project(agents: [agent("dev-1", trees: [AgentWorktree(repo: "docs", path: "/w/docs")], folder: nil)]))
        XCTAssertEqual(overview.repos.map(\.repo), ["docs"])
        XCTAssertNil(overview.repos[0].position)
        XCTAssertNil(overview.repos[0].mergeOrder)
        XCTAssertEqual(overview.reposCount, "Repos · 0 in merge order")
    }

    func testAgentsListTheirWorktreesAndChangeFolder() throws {
        let trees = [AgentWorktree(repo: "api", path: "/w/api", branch: "b"), AgentWorktree(repo: "web", path: "/w/web")]
        let overview = WorkspaceOverview.make(
            try project(agents: [
                agent("dev-4", trees: trees, folder: "/w", branch: "feat/PM-512"), agent("pm", trees: nil, folder: nil),
            ]))
        XCTAssertEqual(overview.agents.map(\.agentId), ["dev-4"], "an agent without worktrees has no group")
        XCTAssertEqual(overview.agents[0].heading, "dev-4 worktrees on feat/PM-512")
        XCTAssertEqual(overview.agents[0].changeFolder, "/w")
        XCTAssertEqual(overview.agents[0].worktrees.count, 2)
    }

    func testAProjectWithoutWorkspaceDataIsEmpty() throws {
        let overview = WorkspaceOverview.make(try project(agents: [agent("dev-1", trees: nil, folder: nil)]))
        XCTAssertTrue(overview.isEmpty)
        XCTAssertEqual(WorkspaceOverview.older, "The host did not send workspace data. It may run a Capstan older than this app.")
        XCTAssertEqual(WorkspaceOverview.loading(host: "build-1"), "Loading the workspace from build-1…")
    }

    func testTheMainBranchAndTheCleanupsComeFromTheRunItem() throws {
        var model = try project(agents: [agent("dev-1", trees: [AgentWorktree(repo: "web", path: "/w/web")], folder: "/w")])
        model.run = StoredRun(
            requestId: "run", state: "active", paused: false, contentHash: "h",
            repos: [WorkspaceRepo(repo: "api", mainBranch: "main"), WorkspaceRepo(repo: "web", mainBranch: "release")],
            cleanups: [
                CleanupOffer(cleanupId: "c-2", repo: "web", branches: 2, worktrees: 1),
                CleanupOffer(cleanupId: "c-1", repo: "old", branches: 1, worktrees: 1, left: true),
            ])
        let overview = WorkspaceOverview.make(model)
        XCTAssertEqual(overview.repos.map(\.repo), ["api", "web"])
        XCTAssertEqual(overview.repos.map(\.mainBranch), ["main", "release"])
        XCTAssertEqual(overview.repos.map(\.worktrees), [0, 1])
        XCTAssertEqual(overview.cleanups.map(\.line), [
            "cleanup c-2 · web: branches 2, worktrees 1", "cleanup c-1 · old: branches 1, worktrees 1",
        ])
        XCTAssertNil(overview.cleanups[0].leftNote)
        XCTAssertNil(overview.cleanups[0].applyCommand)
        XCTAssertEqual(overview.cleanups[1].leftNote, "old left the configuration: worktrees only, apply them by name")
        XCTAssertEqual(overview.cleanups[1].applyCommand, "cstan cleanup apply c-1 <branch>")
        XCTAssertFalse(overview.isEmpty)
    }

    func testAHostWithoutReposOrCleanupsShowsNoMainBranchAndNoCleanups() throws {
        let members = [member("api", 0, "merged")]
        let set = IntegrationSet(setId: "s1", state: "merged", repo: "api", position: 0, members: members)
        let overview = WorkspaceOverview.make(try project(waiting: [waiting("integration:1", set: set)]))
        XCTAssertEqual(overview.repos.map(\.mainBranch), [nil])
        XCTAssertTrue(overview.cleanups.isEmpty)
        XCTAssertEqual(WorkspaceOverview.noCleanups, "No repo has anything to clean up.")
    }
}
