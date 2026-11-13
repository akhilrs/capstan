import CapstanKit
import Foundation

/// The Workspace overview (board 07): the repos of the project in merge order, and the worktrees each agent holds. It
/// reads what the feed carries: the repos with their main branches and the open cleanup offers (the `run` item), the sets
/// of the integration items and the worktrees of the agents. A host before 0.24.0 sends no repos or cleanups; the repos
/// then come from the sets and show no main branch, and the cleanups section is empty.
public struct WorkspaceOverview: Sendable, Equatable {
    public struct RepoRow: Sendable, Equatable, Identifiable {
        public var id: String { repo }
        /// 1-based merge order; nil for a repo that only an agent holds a worktree in.
        public let position: Int?
        public let repo: String
        /// Sets that have a live member in this repo that is not in main.
        public let openSets: Int
        public let worktrees: Int
        /// The repo's main branch from the host's configuration; nil from a host that does not send it.
        public let mainBranch: String?

        public var mergeOrder: String? { position.map { "Merge order \($0)" } }
    }

    public struct AgentWorktrees: Sendable, Equatable, Identifiable {
        public var id: String { agentId }
        public let agentId: String
        /// `dev-4 worktrees on feat/PM-512-refund-sync`, or `dev-4 worktrees` without a branch.
        public let heading: String
        public let worktrees: [AgentWorktree]
        public let changeFolder: String?
    }

    /// One open cleanup offer, as `cleanup.repo` reads it; a repo that left the configuration has the left-note.
    public struct CleanupRow: Sendable, Equatable, Identifiable {
        public var id: String { cleanupId }
        public let cleanupId: String
        public let repo: String
        /// `cleanup <id> · <repo>: branches 2, worktrees 1`.
        public let line: String
        /// `<repo> left the configuration: worktrees only, apply them by name`; nil for a configured repo.
        public let leftNote: String?
        /// The command that applies a left repo's entries by name; nil for a configured repo.
        public let applyCommand: String?
    }

    public let repos: [RepoRow]
    public let cleanups: [CleanupRow]
    public let agents: [AgentWorktrees]
    /// The project has no workspace data at all: a single-repo project or a host that predates workspaces.
    public let isEmpty: Bool

    public static let title = "Workspace"
    public static let reposHeading = "Repos"
    public static let worktreesHeading = "Worktrees"
    public static let cleanupsHeading = "Cleanups by repo"
    public static let noCleanups = "No repo has anything to clean up."
    public static let mainBranchLabel = "Main branch"
    public static let changeFolderLabel = "Change folder"
    public static let noWorktrees = "No worktrees: this agent has not started work in a repo."
    public static let older = "The host did not send workspace data. It may run a Capstan older than this app."

    /// `Repos · 3 in merge order`.
    public var reposCount: String { "Repos · \(repos.filter { $0.position != nil }.count) in merge order" }

    public static func loading(host: String) -> String { "Loading the workspace from " + host + "…" }

    public static func make(_ project: ProjectModel) -> WorkspaceOverview {
        let sets = distinctSets(project)
        let agents = project.agents.values
            .filter { !($0.worktrees ?? []).isEmpty }
            .sorted { $0.agentId < $1.agentId }
        var worktreeCount: [String: Int] = [:]
        for agent in agents { for tree in agent.worktrees ?? [] { worktreeCount[tree.repo, default: 0] += 1 } }
        var mainBranch: [String: String] = [:]
        // The configuration gives the order when the host sends it; else the longest set does.
        var order: [String] = []
        for repo in project.run?.repos ?? [] where !order.contains(repo.repo) {
            order.append(repo.repo)
            mainBranch[repo.repo] = repo.mainBranch
        }
        // A repo the sets name that the configuration lacks follows in the order of the sets.
        for set in sets.sorted(by: { $0.members.count > $1.members.count }) {
            for member in set.members where !order.contains(member.repo) { order.append(member.repo) }
        }
        let positioned = Set(order)
        let others = worktreeCount.keys.filter { !positioned.contains($0) }.sorted()
        let rows = order.enumerated().map { index, repo in
            RepoRow(
                position: index + 1, repo: repo, openSets: sets.filter { open($0, repo: repo) }.count,
                worktrees: worktreeCount[repo] ?? 0, mainBranch: mainBranch[repo])
        } + others.map {
            RepoRow(position: nil, repo: $0, openSets: 0, worktrees: worktreeCount[$0] ?? 0, mainBranch: nil)
        }
        let cleanups = (project.run?.cleanups ?? []).map { offer in
            CleanupRow(
                cleanupId: offer.cleanupId, repo: offer.repo,
                line: "cleanup \(offer.cleanupId) · \(offer.repo): branches \(offer.branches), worktrees \(offer.worktrees)",
                leftNote: offer.left ? offer.repo + " left the configuration: worktrees only, apply them by name" : nil,
                applyCommand: offer.left ? "cstan cleanup apply \(offer.cleanupId) <branch>" : nil)
        }
        let groups = agents.map { agent -> AgentWorktrees in
            let branch = agent.branch.flatMap { $0.verified && !$0.displayPlain.isEmpty ? $0.displayPlain : nil }
            return AgentWorktrees(
                agentId: agent.agentId, heading: agent.agentId + " worktrees" + (branch.map { " on " + $0 } ?? ""),
                worktrees: agent.worktrees ?? [], changeFolder: agent.changeFolder)
        }
        return WorkspaceOverview(
            repos: rows, cleanups: cleanups, agents: groups, isEmpty: rows.isEmpty && groups.isEmpty && cleanups.isEmpty)
    }

    /// One set per set id; the member that is next carries the same set as every other member item.
    static func distinctSets(_ project: ProjectModel) -> [IntegrationSet] {
        var seen: Set<String> = []
        var sets: [IntegrationSet] = []
        for key in project.waiting.keys.sorted() {
            guard let set = project.waiting[key]?.set, seen.insert(set.setId).inserted else { continue }
            sets.append(set)
        }
        return sets
    }

    private static func open(_ set: IntegrationSet, repo: String) -> Bool {
        set.liveMembers.contains { $0.repo == repo && $0.state != "confirmed" && $0.state != "discarded" }
    }
}
