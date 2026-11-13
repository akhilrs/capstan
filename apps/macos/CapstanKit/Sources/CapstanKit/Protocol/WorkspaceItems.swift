import Foundation

// The workspace members of the feed (document section "Workspace members"). Every member is optional on the wire: a
// host without workspace data sends none of them, and a value of the wrong type reads as absent, never as an error, so
// an item from an older or newer host decodes as it does today.

extension JSONValue {
    fileprivate func wsString(_ key: String) -> String? {
        guard case .string(let text)? = optional(key) else { return nil }
        return text
    }

    fileprivate func wsInt(_ key: String) -> Int64? {
        guard case .int(let number)? = optional(key) else { return nil }
        return number
    }

    fileprivate func wsBool(_ key: String) -> Bool? {
        guard case .bool(let flag)? = optional(key) else { return nil }
        return flag
    }

    fileprivate func wsList<T>(_ key: String, _ make: (JSONValue) -> T?) -> [T]? {
        guard case .array(let elements)? = optional(key) else { return nil }
        return elements.compactMap(make)
    }
}

/// One repo of an integration set, in merge order.
public struct SetMember: Codable, Sendable, Equatable {
    public var repo: String
    /// 0 is first in merge order.
    public var position: Int64
    /// 1 for the first integration of the repo, one more for each redo.
    public var attempt: Int64
    /// nil until the repo has an integration.
    public var integrationId: String?
    /// `running`, `merged` (built: merged into the integration branch only), `confirmed`, `conflicted`, `failed` or
    /// `discarded`; kept as the host wrote it.
    public var state: String
    public var branch: String?
    /// 7 to 64 lowercase hex characters; any other value is dropped.
    public var headSha: String?
    /// The member was discarded while the set goes on: outside the gate, the review and the confirm.
    public var dropped: Bool

    public init(
        repo: String, position: Int64, attempt: Int64 = 1, integrationId: String? = nil, state: String,
        branch: String? = nil, headSha: String? = nil, dropped: Bool = false
    ) {
        self.repo = repo
        self.position = position
        self.attempt = attempt
        self.integrationId = integrationId
        self.state = state
        self.branch = branch
        self.headSha = headSha
        self.dropped = dropped
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let repo = json.wsString("repo"), let position = json.wsInt("position"),
              let state = json.wsString("state") else { return nil }
        self.init(
            repo: repo, position: position, attempt: max(1, json.wsInt("attempt") ?? 1),
            integrationId: json.wsString("integrationId"), state: state, branch: json.wsString("branch"),
            headSha: json.wsString("headSha").flatMap(WaitingItem.validSha), dropped: json.wsBool("dropped") ?? false)
    }

    var json: JSONValue {
        var members: [String: JSONValue] = [
            "repo": .string(repo), "position": .int(position), "attempt": .int(attempt), "state": .string(state),
            "dropped": .bool(dropped),
        ]
        if let integrationId { members["integrationId"] = .string(integrationId) }
        if let branch { members["branch"] = .string(branch) }
        if let headSha { members["headSha"] = .string(headSha) }
        return .object(members)
    }

    /// A redo has been made: the screens then say "attempt n".
    public var isRedone: Bool { attempt > 1 }
}

/// The integration set an integration item belongs to, from the point of view of that member (`set` of the feed).
public struct IntegrationSet: Codable, Sendable, Equatable {
    public var setId: String
    /// The set's own state: `running`, `merged`, `conflicted`, `failed` or `discarded`.
    public var state: String
    /// This member's repo, position and attempt.
    public var repo: String
    public var position: Int64
    public var attempt: Int64
    /// This member is the one the set's review is about.
    public var reviewSubject: Bool
    public var dropped: Bool
    /// Every repo in merge order, at most 32.
    public var members: [SetMember]

    public static let memberCap = 32

    public init(
        setId: String, state: String, repo: String, position: Int64, attempt: Int64 = 1, reviewSubject: Bool = false,
        dropped: Bool = false, members: [SetMember]
    ) {
        self.setId = setId
        self.state = state
        self.repo = repo
        self.position = position
        self.attempt = attempt
        self.reviewSubject = reviewSubject
        self.dropped = dropped
        self.members = members
    }

    /// Nil when the value is not an object with a set id, a state, a repo and a position. Members that do not read are
    /// dropped; the rest are put in merge order and cut at 32.
    init?(wire json: JSONValue) {
        guard case .object = json, let setId = json.wsString("setId"), let state = json.wsString("state"),
              let repo = json.wsString("repo"), let position = json.wsInt("position") else { return nil }
        let list = (json.wsList("members") { SetMember(wire: $0) } ?? [])
            .sorted { $0.position < $1.position }
            .prefix(IntegrationSet.memberCap)
        self.init(
            setId: setId, state: state, repo: repo, position: position, attempt: max(1, json.wsInt("attempt") ?? 1),
            reviewSubject: json.wsBool("reviewSubject") ?? false, dropped: json.wsBool("dropped") ?? false,
            members: Array(list))
    }

    var json: JSONValue {
        .object([
            "setId": .string(setId), "state": .string(state), "repo": .string(repo), "position": .int(position),
            "attempt": .int(attempt), "reviewSubject": .bool(reviewSubject), "dropped": .bool(dropped),
            "members": .array(members.map(\.json)),
        ])
    }

    /// The members that count: a dropped member is outside the gate, the review and the confirm.
    public var liveMembers: [SetMember] { members.filter { !$0.dropped } }

    /// The member of this item.
    public var member: SetMember? { members.first { $0.repo == repo } }

    /// The first live member that is not yet in main. The host refuses any other with "merge <x> first". A member
    /// that is still running, conflicted or failed has nothing to merge yet, but it is still the one that is next.
    public var next: SetMember? { liveMembers.first { $0.state != "confirmed" && $0.state != "discarded" } }

    /// The live members that are in main.
    public var inMainCount: Int { liveMembers.filter { $0.state == "confirmed" }.count }

    /// The live members that are built (merged into the integration branch) or in main.
    public var builtCount: Int { liveMembers.filter { $0.state == "merged" || $0.state == "confirmed" }.count }

    public var droppedCount: Int { members.filter(\.dropped).count }

    /// Every live member is in main.
    public var isInMain: Bool { !liveMembers.isEmpty && inMainCount == liveMembers.count }

    /// The member that has to be merged before `repo`, if any: the first live member before it that is not in main.
    public func blocker(of repo: String) -> SetMember? {
        guard let target = members.first(where: { $0.repo == repo }) else { return nil }
        return liveMembers.first { $0.position < target.position && $0.state != "confirmed" && $0.state != "discarded" }
    }
}

/// The latest and the last passed verify run of one repo of a set (`verify.byRepo` of the feed).
public struct RepoVerify: Codable, Sendable, Equatable {
    public var repo: String
    /// nil while the repo has no finished run.
    public var latest: IntegrationVerifyRun?
    public var lastPassed: IntegrationVerifyPassed?

    public init(repo: String, latest: IntegrationVerifyRun? = nil, lastPassed: IntegrationVerifyPassed? = nil) {
        self.repo = repo
        self.latest = latest
        self.lastPassed = lastPassed
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let repo = json.wsString("repo") else { return nil }
        self.init(
            repo: repo, latest: json.optional("latest").flatMap { IntegrationVerifyRun(wire: $0) },
            lastPassed: json.optional("lastPassed").flatMap { IntegrationVerifyPassed(wire: $0) })
    }

    /// The list a `verify` member carries, nil when it has none. At most 32 rows are read.
    static func list(in verify: JSONValue?) -> [RepoVerify]? {
        guard let verify, let rows = verify.wsList("byRepo", { RepoVerify(wire: $0) }) else { return nil }
        return Array(rows.prefix(IntegrationSet.memberCap))
    }

    var json: JSONValue {
        var members: [String: JSONValue] = ["repo": .string(repo)]
        if let latest {
            members["latest"] = .object([
                "state": .string(latest.state), "commitSha": .string(latest.commitSha),
                "finishedAt": .string(latest.finishedAt),
            ])
        }
        if let lastPassed {
            members["lastPassed"] = .object([
                "commitSha": .string(lastPassed.commitSha), "finishedAt": .string(lastPassed.finishedAt),
            ])
        }
        return .object(members)
    }
}

/// One commit of a report, in one repo.
public struct ReportCommit: Codable, Sendable, Equatable {
    public var repo: String
    public var commitSha: String
    public var branch: String?
    public var baseSha: String?

    public init(repo: String, commitSha: String, branch: String? = nil, baseSha: String? = nil) {
        self.repo = repo
        self.commitSha = commitSha
        self.branch = branch
        self.baseSha = baseSha
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let repo = json.wsString("repo"),
              let sha = json.wsString("commitSha").flatMap(WaitingItem.validSha) else { return nil }
        self.init(
            repo: repo, commitSha: sha, branch: json.wsString("branch"),
            baseSha: json.wsString("baseSha").flatMap(WaitingItem.validSha))
    }

    /// The commits of a `commits` member, nil when there is none (a report that named no commits has no member).
    static func list(in json: JSONValue) -> [ReportCommit]? {
        guard let rows = json.wsList("commits", { ReportCommit(wire: $0) }), !rows.isEmpty else { return nil }
        return rows
    }
}

/// A worktree an agent holds in one repo.
public struct AgentWorktree: Codable, Sendable, Equatable {
    public var repo: String
    public var path: String
    public var branch: String?

    public init(repo: String, path: String, branch: String? = nil) {
        self.repo = repo
        self.path = path
        self.branch = branch
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let repo = json.wsString("repo"), let path = json.wsString("path") else { return nil }
        self.init(repo: repo, path: path, branch: json.wsString("branch"))
    }

    static func list(in json: JSONValue) -> [AgentWorktree]? {
        guard let rows = json.wsList("worktrees", { AgentWorktree(wire: $0) }), !rows.isEmpty else { return nil }
        return rows
    }

    var json: JSONValue {
        var members: [String: JSONValue] = ["repo": .string(repo), "path": .string(path)]
        if let branch { members["branch"] = .string(branch) }
        return .object(members)
    }
}

extension ReportCommit {
    var json: JSONValue {
        var members: [String: JSONValue] = ["repo": .string(repo), "commitSha": .string(commitSha)]
        if let branch { members["branch"] = .string(branch) }
        if let baseSha { members["baseSha"] = .string(baseSha) }
        return .object(members)
    }
}

extension EventBody {
    /// The set of an `integration.finished` event of a member; nil for another event and from an older host.
    public var integrationSet: IntegrationSet? {
        guard type == "integration.finished" else { return nil }
        return raw.optional("set").flatMap { IntegrationSet(wire: $0) }
    }

    /// The commits of a `report.recorded` event, one per repo; nil when the report named none.
    public var reportCommits: [ReportCommit]? {
        guard type == "report.recorded" else { return nil }
        return ReportCommit.list(in: raw)
    }
}

/// A repo of the workspace with its main branch, from the `repos` member of the run item (merge order).
public struct WorkspaceRepo: Codable, Sendable, Equatable {
    public var repo: String
    public var mainBranch: String

    public init(repo: String, mainBranch: String) {
        self.repo = repo
        self.mainBranch = mainBranch
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let repo = json.wsString("repo"), let main = json.wsString("mainBranch"), !main.isEmpty
        else { return nil }
        self.init(repo: repo, mainBranch: main)
    }

    /// The repos of a `repos` member, nil when there is none (a single-repo project or a host before it).
    static func list(in json: JSONValue) -> [WorkspaceRepo]? {
        guard let rows = json.wsList("repos", { WorkspaceRepo(wire: $0) }), !rows.isEmpty else { return nil }
        return rows
    }

    var json: JSONValue { .object(["repo": .string(repo), "mainBranch": .string(mainBranch)]) }
}

/// One open cleanup offer of a repo, from the `cleanups` member of the run item.
public struct CleanupOffer: Codable, Sendable, Equatable {
    public var cleanupId: String
    public var repo: String
    /// The branches still offered, and how many of them have a worktree.
    public var branches: Int64
    public var worktrees: Int64
    /// The repo left the configuration: its entries go only by name.
    public var left: Bool

    public init(cleanupId: String, repo: String, branches: Int64, worktrees: Int64, left: Bool = false) {
        self.cleanupId = cleanupId
        self.repo = repo
        self.branches = branches
        self.worktrees = worktrees
        self.left = left
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let id = json.wsString("cleanupId"), let repo = json.wsString("repo") else { return nil }
        self.init(
            cleanupId: id, repo: repo, branches: json.wsInt("branches") ?? 0, worktrees: json.wsInt("worktrees") ?? 0,
            left: json.wsBool("left") ?? false)
    }

    /// The offers of a `cleanups` member, nil when there is none.
    static func list(in json: JSONValue) -> [CleanupOffer]? {
        guard let rows = json.wsList("cleanups", { CleanupOffer(wire: $0) }), !rows.isEmpty else { return nil }
        return rows
    }

    var json: JSONValue {
        var members: [String: JSONValue] = [
            "cleanupId": .string(cleanupId), "repo": .string(repo), "branches": .int(branches), "worktrees": .int(worktrees),
        ]
        if left { members["left"] = .bool(true) }
        return .object(members)
    }
}
