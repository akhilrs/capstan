import Foundation

// Codable forms of the protocol values the store keeps. The wire enums carry an `other` case for names a later
// version may add, so they are written as their wire names.

extension ProjectAvailability: Codable {
    public var wireName: String {
        switch self {
        case .ready: return "ready"
        case .noKey: return "no_key"
        case .silent: return "silent"
        case .refused: return "refused"
        case .unsupported: return "unsupported"
        case .gone: return "gone"
        case .other(let name): return name
        }
    }

    public init(from decoder: Decoder) throws {
        self.init(wire: try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(wireName)
    }
}

extension WaitingKind: Codable {
    public var wireName: String {
        switch self {
        case .op: return "op"
        case .prompt: return "prompt"
        case .design: return "design"
        case .message: return "message"
        case .integration: return "integration"
        case .other(let name): return name
        }
    }

    public init(from decoder: Decoder) throws {
        self.init(wire: try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(wireName)
    }
}

extension PackageState: Codable {
    public var wireName: String {
        switch self {
        case .unassigned: return "unassigned"
        case .assigned: return "assigned"
        case .reported: return "reported"
        case .integrated: return "integrated"
        case .confirmed: return "confirmed"
        case .cancelled: return "cancelled"
        case .other(let name): return name
        }
    }

    public init(from decoder: Decoder) throws {
        self.init(wire: try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(wireName)
    }
}

extension PackageDetail {
    var json: JSONValue {
        var members: [String: JSONValue] = [
            "id": .string(id), "title": .string(title.raw), "role": .string(role), "state": .string(state.wireName),
            "dependsOn": .array(dependsOn.map { JSONValue.string($0) }),
        ]
        if let assigneeAgentId { members["assigneeAgentId"] = .string(assigneeAgentId) }
        if let review { members["review"] = .string(review) }
        if let integrationId { members["integrationId"] = .string(integrationId) }
        if let integrationState { members["integrationState"] = .string(integrationState) }
        return .object(members)
    }
}

extension Cursor: Codable {
    public init(from decoder: Decoder) throws {
        let text = try decoder.singleValueContainer().decode(String.self)
        guard let cursor = Cursor(text: text) else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "cursor text"))
        }
        self = cursor
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(text)
    }
}

// MARK: Items

public struct StoredWaiting: Codable, Sendable, Equatable {
    public var key: String
    public var requestId: String
    public var kind: WaitingKind
    public var since: String
    public var summary: UntrustedText
    public var contentHash: String
    public var needsTerminal: Bool
    /// Filled for kind integration; saved files from before the host sent these decode them as nil.
    public var integrationState: String?
    public var branch: UntrustedText?
    public var headSha: String?
    /// Saved files from before the host sent the details decode them as nil.
    public var integrationDetails: IntegrationDetails?

    public init(
        key: String, requestId: String, kind: WaitingKind, since: String, summary: UntrustedText,
        contentHash: String, needsTerminal: Bool, integrationState: String? = nil, branch: UntrustedText? = nil,
        headSha: String? = nil, integrationDetails: IntegrationDetails? = nil
    ) {
        self.integrationDetails = integrationDetails
        self.integrationState = integrationState
        self.branch = branch
        self.headSha = headSha
        self.key = key
        self.requestId = requestId
        self.kind = kind
        self.since = since
        self.summary = summary
        self.contentHash = contentHash
        self.needsTerminal = needsTerminal
    }

    init(_ item: WaitingItem) {
        self.init(
            key: item.key, requestId: item.requestId, kind: item.kind, since: item.since, summary: item.summary,
            contentHash: item.contentHash, needsTerminal: item.needsTerminal, integrationState: item.integrationState,
            branch: item.branch, headSha: item.headSha, integrationDetails: item.integrationDetails)
    }

    var json: JSONValue {
        var members: [String: JSONValue] = [
            "key": .string(key), "requestId": .string(requestId), "kind": .string(kind.wireName),
            "since": .string(since), "summary": .string(summary.raw), "contentHash": .string(contentHash),
            "needsTerminal": .bool(needsTerminal),
        ]
        if let integrationState { members["integrationState"] = .string(integrationState) }
        if let branch { members["branch"] = .string(branch.raw) }
        if let headSha { members["headSha"] = .string(headSha) }
        if let integrationDetails { members["integrationDetails"] = integrationDetails.json }
        return .object(members)
    }
}

/// A text member with its flags beside it, as the host writes it.
private func flagged(_ key: String, _ text: UntrustedText, _ extra: [String: JSONValue] = [:]) -> JSONValue {
    var members = extra
    members[key] = .string(text.raw)
    members["textVerified"] = .bool(text.verified)
    members["truncated"] = .bool(text.truncated)
    return .object(members)
}

extension IntegrationDetails {
    /// The wire form, so a stored item read back through `WaitingItem(json:)` keeps its details and their flags.
    var json: JSONValue {
        if !recorded { return .object(["recorded": .bool(false)]) }
        let planList: [JSONValue] = plans.map { flagged("title", $0.title, ["planId": .string($0.planId)]) }
        let reportList: [JSONValue] = reports.map { (report: IntegrationReport) -> JSONValue in
            var extra: [String: JSONValue] = ["reportId": .string(report.reportId)]
            if let packageId = report.packageId { extra["packageId"] = .string(packageId) }
            return flagged("summary", report.summary, extra)
        }
        let signoffList: [JSONValue] = signoffs.map { (signoff: IntegrationSignoff) -> JSONValue in
            let extra: [String: JSONValue] = ["planId": .string(signoff.planId), "createdAt": .string(signoff.createdAt)]
            return flagged("summary", signoff.summary, extra)
        }
        var members: [String: JSONValue] = [
            "recorded": .bool(true), "plans": .array(planList), "plansOmitted": .int(plansOmitted),
            "reports": .array(reportList), "reportsOmitted": .int(reportsOmitted),
            "signoffs": .array(signoffList), "signoffsOmitted": .int(signoffsOmitted),
        ]
        if let impactError { members["impactError"] = .string(impactError) }
        if let impact { members["impact"] = impact.json }
        if let review {
            members["review"] = .object([
                "verdict": .string(review.verdict), "reviewerAgentId": .string(review.reviewerAgentId),
                "round": .int(review.round),
            ])
        }
        if let verify { members["verify"] = verify.json }
        return .object(members)
    }
}

extension IntegrationImpact {
    var json: JSONValue {
        let areaList: [JSONValue] = areas.map { flagged("path", $0.path, ["files": .int($0.files)]) }
        let migrationList: [JSONValue] = migrations.map { flagged("path", $0.path) }
        let stepList: [JSONValue] = nextSteps.map { flagged("text", $0.text) }
        return .object([
            "filesChanged": .int(filesChanged), "insertions": .int(insertions), "deletions": .int(deletions),
            "truncated": .bool(truncated), "areas": .array(areaList), "areasOmitted": .int(areasOmitted),
            "migrations": .array(migrationList), "migrationsOmitted": .int(migrationsOmitted),
            "nextSteps": .array(stepList),
        ])
    }
}

extension IntegrationVerify {
    var json: JSONValue {
        var members: [String: JSONValue] = [
            "latest": .object([
                "state": .string(latest.state), "commitSha": .string(latest.commitSha),
                "finishedAt": .string(latest.finishedAt),
            ])
        ]
        if let lastPassed {
            members["lastPassed"] = .object([
                "commitSha": .string(lastPassed.commitSha), "finishedAt": .string(lastPassed.finishedAt),
            ])
        }
        return .object(members)
    }
}

public struct StoredAgent: Codable, Sendable, Equatable {
    public var key: String
    public var agentId: String
    public var kind: String
    public var role: String
    public var generation: Int64
    public var state: String
    public var paused: Bool
    public var task: UntrustedText?
    /// The branch of the agent's pane; saved files from before the host sent it decode it as nil.
    public var branch: UntrustedText?
    public var requestId: String
    public var contentHash: String
    /// The host's activity text, its time and sequence; saved files from before them decode all three as nil.
    public var activity: String?
    public var activitySince: String?
    public var activitySeq: Int64?

    public init(
        key: String, agentId: String, kind: String, role: String, generation: Int64, state: String, paused: Bool,
        task: UntrustedText?, requestId: String, contentHash: String, branch: UntrustedText? = nil,
        activity: String? = nil, activitySince: String? = nil, activitySeq: Int64? = nil
    ) {
        self.activity = activity
        self.activitySince = activitySince
        self.activitySeq = activitySeq
        self.branch = branch
        self.key = key
        self.agentId = agentId
        self.kind = kind
        self.role = role
        self.generation = generation
        self.state = state
        self.paused = paused
        self.task = task
        self.requestId = requestId
        self.contentHash = contentHash
    }

    init(_ item: AgentItem) {
        self.init(
            key: item.key, agentId: item.agentId, kind: item.kind, role: item.role, generation: item.generation,
            state: item.state, paused: item.paused, task: item.task, requestId: item.requestId,
            contentHash: item.contentHash, branch: item.branch, activity: item.activity,
            activitySince: item.activitySince, activitySeq: item.activitySeq)
    }

    var json: JSONValue {
        var members: [String: JSONValue] = [
            "key": .string(key), "agentId": .string(agentId), "kind": .string(kind), "role": .string(role),
            "generation": .int(generation), "state": .string(state), "paused": .bool(paused),
            "requestId": .string(requestId), "contentHash": .string(contentHash),
        ]
        if let task { members["task"] = .string(task.raw) }
        if let branch { members["branch"] = .string(branch.raw) }
        if let activity { members["activity"] = .string(activity) }
        if let activitySince { members["activitySince"] = .string(activitySince) }
        if let activitySeq { members["activitySeq"] = .int(activitySeq) }
        return .object(members)
    }
}

/// Package counts of a plan; `assigned` includes `reported`, which includes `integrated`.
public struct StoredCounts: Codable, Sendable, Equatable {
    public var total: Int64
    public var assigned: Int64
    public var reported: Int64
    public var integrated: Int64

    public init(total: Int64, assigned: Int64, reported: Int64, integrated: Int64) {
        self.total = total
        self.assigned = assigned
        self.reported = reported
        self.integrated = integrated
    }
}

public struct StoredPlan: Codable, Sendable, Equatable {
    public var key: String
    public var planId: String
    public var sequence: Int64
    public var title: UntrustedText
    public var state: String
    public var packages: StoredCounts
    /// Nil from a host that does not send them, and in saved files from before it did.
    public var packageDetails: [PackageDetail]?
    public var packageDetailsOmitted: Int64?

    public init(
        key: String, planId: String, sequence: Int64, title: UntrustedText, state: String, packages: StoredCounts,
        packageDetails: [PackageDetail]? = nil, packageDetailsOmitted: Int64? = nil
    ) {
        self.packageDetails = packageDetails
        self.packageDetailsOmitted = packageDetailsOmitted
        self.key = key
        self.planId = planId
        self.sequence = sequence
        self.title = title
        self.state = state
        self.packages = packages
    }

    init(_ item: PlanItem) {
        self.init(
            key: item.key, planId: item.planId, sequence: item.sequence, title: item.title, state: item.state,
            packages: StoredCounts(
                total: item.packages.total, assigned: item.packages.assigned, reported: item.packages.reported,
                integrated: item.packages.integrated),
            packageDetails: item.packageDetails, packageDetailsOmitted: item.packageDetailsOmitted)
    }

    var json: JSONValue {
        var members: [String: JSONValue] = [
            "key": .string(key), "planId": .string(planId), "sequence": .int(sequence), "title": .string(title.raw),
            "state": .string(state),
            "packages": .object([
                "total": .int(packages.total), "assigned": .int(packages.assigned),
                "reported": .int(packages.reported), "integrated": .int(packages.integrated),
            ]),
        ]
        if let packageDetails { members["packageDetails"] = .array(packageDetails.map { $0.json }) }
        if let packageDetailsOmitted { members["packageDetailsOmitted"] = .int(packageDetailsOmitted) }
        return .object(members)
    }
}

public struct StoredRun: Codable, Sendable, Equatable {
    public var requestId: String
    public var state: String
    public var paused: Bool
    public var contentHash: String

    public init(requestId: String, state: String, paused: Bool, contentHash: String) {
        self.requestId = requestId
        self.state = state
        self.paused = paused
        self.contentHash = contentHash
    }

    /// False between a `run.changed` event and the read that follows it: a decision would carry a stale hash.
    public var hashKnown: Bool { !contentHash.isEmpty }

    init(_ item: RunItem) {
        self.init(requestId: item.requestId, state: item.state, paused: item.paused, contentHash: item.contentHash)
    }

    var json: JSONValue {
        .object([
            "requestId": .string(requestId), "state": .string(state), "paused": .bool(paused),
            "contentHash": .string(contentHash),
        ])
    }
}

/// The latest event that named a waiting key: what the snapshot item does not hold, such as the full command or the
/// options of a prompt. Only events seen live are kept.
public struct ItemDetail: Codable, Sendable, Equatable {
    public var key: String
    public var eventType: String
    public var seq: Int64
    /// The body of the event as the host sent it.
    public var body: JSONValue

    public init(key: String, eventType: String, seq: Int64, body: JSONValue) {
        self.key = key
        self.eventType = eventType
        self.seq = seq
        self.body = body
    }

    private var view: EventBody? {
        try? EventBody(type: eventType, raw: body)
    }

    /// False when the host could not vouch for the text of the event.
    public var textVerified: Bool {
        if case .bool(let flag)? = body["textVerified"] { return flag }
        return true
    }

    public var operatorBody: OperatorBody? { view?.operatorBody }
    public var promptBody: PromptBody? { view?.promptBody }
    public var messageBody: MessageBody? { view?.messageBody }
    public var designBody: DesignBody? { view?.designBody }
}

// MARK: Models

public struct HostHello: Codable, Sendable, Equatable {
    public var version: UntrustedText
    public var host: UntrustedText
    public var projects: Int

    public init(version: UntrustedText, host: UntrustedText, projects: Int) {
        self.version = version
        self.host = host
        self.projects = projects
    }
}

public struct ProjectModel: Codable, Sendable, Equatable {
    public var projectId: String
    public var root: String
    public var availability: ProjectAvailability
    public var epoch: String?
    public var head: Int64?
    public var waiting: [String: StoredWaiting]
    public var agents: [String: StoredAgent]
    public var plans: [String: StoredPlan]
    public var run: StoredRun?
    /// By waiting key.
    public var details: [String: ItemDetail]
    /// The kind of attention an agent needs, by agent id (for example `stalled`).
    public var attention: [String: String]
    public var snapshotComplete: Bool
    /// Since when the project has not been `ready`; its items are the last ones read.
    public var staleSince: Date?

    public init(
        projectId: String, root: String, availability: ProjectAvailability = .ready, epoch: String? = nil,
        head: Int64? = nil, waiting: [String: StoredWaiting] = [:], agents: [String: StoredAgent] = [:],
        plans: [String: StoredPlan] = [:], run: StoredRun? = nil, details: [String: ItemDetail] = [:],
        attention: [String: String] = [:], snapshotComplete: Bool = false, staleSince: Date? = nil
    ) {
        self.projectId = projectId
        self.root = root
        self.availability = availability
        self.epoch = epoch
        self.head = head
        self.waiting = waiting
        self.agents = agents
        self.plans = plans
        self.run = run
        self.details = details
        self.attention = attention
        self.snapshotComplete = snapshotComplete
        self.staleSince = staleSince
    }

    /// The project can be resumed from a saved cursor: its snapshot is complete and its run, if any, has a hash. A run
    /// whose hash is unknown (a `run.changed` not yet read) is read again in full instead, which brings the hash.
    var isResumable: Bool { snapshotComplete && (run?.hashKnown ?? true) }

    /// The last path component of the root; the project id when the root has none.
    public var displayName: String {
        ProjectModel.displayName(root: root, fallback: projectId)
    }

    public static func displayName(root: String, fallback: String) -> String {
        let last = root.split(separator: "/", omittingEmptySubsequences: true).last
        return last.map(String.init) ?? fallback
    }
}

public struct HostModel: Codable, Sendable, Equatable {
    public var hostId: UUID
    public var hello: HostHello?
    public var projects: [String: ProjectModel]
    public var cursor: Cursor?
    /// Forwarded decisions, by project id.
    public var forwards: [String: [ForwardRecord]]

    public init(
        hostId: UUID, hello: HostHello? = nil, projects: [String: ProjectModel] = [:], cursor: Cursor? = nil,
        forwards: [String: [ForwardRecord]] = [:]
    ) {
        self.hostId = hostId
        self.hello = hello
        self.projects = projects
        self.cursor = cursor
        self.forwards = forwards
    }

    public func displayName(_ project: ProjectModel) -> String {
        project.displayName
    }
}
