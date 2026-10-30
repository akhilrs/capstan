import Foundation

/// A value read from the paged snapshot answers (document section 5).
public protocol WireItem: Sendable, Equatable {
    init(json: JSONValue) throws
}

/// Text of a snapshot item: the host sent it plain, so it counts as verified.
private func itemText(_ value: String) -> UntrustedText {
    UntrustedText(raw: value, verified: true, truncated: false)
}

public enum WaitingKind: Sendable, Equatable {
    case op, prompt, design, message, integration
    case other(String)

    public init(wire: String) {
        switch wire {
        case "op": self = .op
        case "prompt": self = .prompt
        case "design": self = .design
        case "message": self = .message
        case "integration": self = .integration
        default: self = .other(wire)
        }
    }
}

public struct WaitingItem: WireItem {
    public let key: String
    public let requestId: String
    public let kind: WaitingKind
    public let since: String
    public let summary: UntrustedText
    public let contentHash: String
    public let needsTerminal: Bool
    /// `merged` or `conflicted`; filled for kind integration, nil otherwise and from a host that does not send it.
    public let integrationState: String?
    public let branch: UntrustedText?
    /// 7 to 64 lowercase hex characters; any other value is dropped.
    public let headSha: String?
    /// What an integration brought and how its checks went; nil for another kind and from a host that does not send it.
    public let integrationDetails: IntegrationDetails?

    public init(json: JSONValue) throws {
        _ = try json.members("waiting item")
        key = try json.string("key")
        requestId = try json.string("requestId")
        kind = WaitingKind(wire: try json.string("kind"))
        since = try json.string("since")
        summary = itemText(try json.string("summary"))
        contentHash = try json.string("contentHash")
        needsTerminal = try json.bool("needsTerminal")
        integrationState = try json.optionalString("integrationState")
        branch = try json.optionalString("branch").map(itemText)
        headSha = try json.optionalString("headSha").flatMap(WaitingItem.validSha)
        integrationDetails = json.optional("integrationDetails").flatMap { IntegrationDetails(wire: $0) }
    }

    static func validSha(_ text: String) -> String? {
        let hex = text.utf8.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
        return hex && (7...64).contains(text.utf8.count) ? text : nil
    }
}

// MARK: Integration details (document section 5)

/// A plan the integration's commit names.
public struct IntegrationPlan: Codable, Sendable, Equatable {
    public var planId: String
    public var title: UntrustedText

    public init(planId: String, title: UntrustedText) {
        self.planId = planId
        self.title = title
    }

    init?(wire json: JSONValue) {
        guard let planId = json.wireString("planId"), let title = json.flaggedText("title") else { return nil }
        self.init(planId: planId, title: title)
    }
}

public struct IntegrationReport: Codable, Sendable, Equatable {
    public var reportId: String
    public var packageId: String?
    public var summary: UntrustedText

    public init(reportId: String, packageId: String? = nil, summary: UntrustedText) {
        self.reportId = reportId
        self.packageId = packageId
        self.summary = summary
    }

    init?(wire json: JSONValue) {
        guard let reportId = json.wireString("reportId"), let summary = json.flaggedText("summary") else { return nil }
        self.init(reportId: reportId, packageId: json.wireString("packageId"), summary: summary)
    }
}

public struct IntegrationSignoff: Codable, Sendable, Equatable {
    public var planId: String
    public var summary: UntrustedText
    public var createdAt: String

    public init(planId: String, summary: UntrustedText, createdAt: String) {
        self.planId = planId
        self.summary = summary
        self.createdAt = createdAt
    }

    init?(wire json: JSONValue) {
        guard let planId = json.wireString("planId"), let summary = json.flaggedText("summary"),
              let createdAt = json.wireString("createdAt") else { return nil }
        self.init(planId: planId, summary: summary, createdAt: createdAt)
    }
}

public struct IntegrationArea: Codable, Sendable, Equatable {
    public var path: UntrustedText
    public var files: Int64

    public init(path: UntrustedText, files: Int64) {
        self.path = path
        self.files = files
    }

    init?(wire json: JSONValue) {
        guard let path = json.flaggedText("path"), let files = json.wireInt("files") else { return nil }
        self.init(path: path, files: files)
    }
}

public struct IntegrationMigration: Codable, Sendable, Equatable {
    public var path: UntrustedText

    public init(path: UntrustedText) {
        self.path = path
    }

    init?(wire json: JSONValue) {
        guard let path = json.flaggedText("path") else { return nil }
        self.init(path: path)
    }
}

/// A next step the host names for the person, as host text.
public struct IntegrationNextStep: Codable, Sendable, Equatable {
    public var text: UntrustedText

    public init(text: UntrustedText) {
        self.text = text
    }

    init?(wire json: JSONValue) {
        guard let text = json.flaggedText("text") else { return nil }
        self.init(text: text)
    }
}

/// The impact of the diff; counts stay non-negative and a list the host left out counts in its `Omitted` member.
public struct IntegrationImpact: Codable, Sendable, Equatable {
    public var filesChanged: Int64
    public var insertions: Int64
    public var deletions: Int64
    /// True when the diff was cut short, so the counts are a lower bound.
    public var truncated: Bool
    public var areas: [IntegrationArea]
    public var areasOmitted: Int64
    public var migrations: [IntegrationMigration]
    public var migrationsOmitted: Int64
    public var nextSteps: [IntegrationNextStep]

    public init(
        filesChanged: Int64, insertions: Int64, deletions: Int64, truncated: Bool = false,
        areas: [IntegrationArea] = [], areasOmitted: Int64 = 0, migrations: [IntegrationMigration] = [],
        migrationsOmitted: Int64 = 0, nextSteps: [IntegrationNextStep] = []
    ) {
        self.filesChanged = filesChanged
        self.insertions = insertions
        self.deletions = deletions
        self.truncated = truncated
        self.areas = areas
        self.areasOmitted = areasOmitted
        self.migrations = migrations
        self.migrationsOmitted = migrationsOmitted
        self.nextSteps = nextSteps
    }

    init?(wire json: JSONValue) {
        guard case .object = json, let files = json.wireInt("filesChanged"), let added = json.wireInt("insertions"),
              let removed = json.wireInt("deletions") else { return nil }
        self.init(
            filesChanged: max(0, files), insertions: max(0, added), deletions: max(0, removed),
            truncated: json.wireBool("truncated") ?? false,
            areas: json.wireList("areas", { IntegrationArea(wire: $0) }), areasOmitted: json.wireCount("areasOmitted"),
            migrations: json.wireList("migrations", { IntegrationMigration(wire: $0) }),
            migrationsOmitted: json.wireCount("migrationsOmitted"),
            nextSteps: json.wireList("nextSteps", { IntegrationNextStep(wire: $0) }))
    }
}

/// The finished review of the integration with the greatest round.
public struct IntegrationReview: Codable, Sendable, Equatable {
    /// `passed`, `findings`, `failed` or `cancelled`; kept as the host wrote it.
    public var verdict: String
    public var reviewerAgentId: String
    public var round: Int64

    public init(verdict: String, reviewerAgentId: String, round: Int64) {
        self.verdict = verdict
        self.reviewerAgentId = reviewerAgentId
        self.round = round
    }

    init?(wire json: JSONValue) {
        guard let verdict = json.wireString("verdict"), let reviewer = json.wireString("reviewerAgentId"),
              let round = json.wireInt("round") else { return nil }
        self.init(verdict: verdict, reviewerAgentId: reviewer, round: round)
    }
}

/// The latest finished verify run of the integration.
public struct IntegrationVerifyRun: Codable, Sendable, Equatable {
    /// `passed`, `failed`, `timeout` or `error`; kept as the host wrote it.
    public var state: String
    public var commitSha: String
    public var finishedAt: String

    public init(state: String, commitSha: String, finishedAt: String) {
        self.state = state
        self.commitSha = commitSha
        self.finishedAt = finishedAt
    }

    init?(wire json: JSONValue) {
        guard let state = json.wireString("state"), let sha = json.wireString("commitSha").flatMap(WaitingItem.validSha),
              let finishedAt = json.wireString("finishedAt") else { return nil }
        self.init(state: state, commitSha: sha, finishedAt: finishedAt)
    }
}

/// The latest verify run that passed.
public struct IntegrationVerifyPassed: Codable, Sendable, Equatable {
    public var commitSha: String
    public var finishedAt: String

    public init(commitSha: String, finishedAt: String) {
        self.commitSha = commitSha
        self.finishedAt = finishedAt
    }

    init?(wire json: JSONValue) {
        guard let sha = json.wireString("commitSha").flatMap(WaitingItem.validSha),
              let finishedAt = json.wireString("finishedAt") else { return nil }
        self.init(commitSha: sha, finishedAt: finishedAt)
    }
}

public struct IntegrationVerify: Codable, Sendable, Equatable {
    public var latest: IntegrationVerifyRun
    public var lastPassed: IntegrationVerifyPassed?

    public init(latest: IntegrationVerifyRun, lastPassed: IntegrationVerifyPassed? = nil) {
        self.latest = latest
        self.lastPassed = lastPassed
    }

    init?(wire json: JSONValue) {
        guard let latestJSON = json.optional("latest"), let latest = IntegrationVerifyRun(wire: latestJSON) else {
            return nil
        }
        self.init(latest: latest, lastPassed: json.optional("lastPassed").flatMap { IntegrationVerifyPassed(wire: $0) })
    }
}

/// What an integration brought and how its checks went. `recorded` false is an integration with no stored details (it
/// was built before they were kept, or it conflicted); every other member is then empty.
public struct IntegrationDetails: Codable, Sendable, Equatable {
    public var recorded: Bool
    /// `timeout` or `diff_failed` when the impact could not be worked out.
    public var impactError: String?
    public var plans: [IntegrationPlan]
    public var plansOmitted: Int64
    public var reports: [IntegrationReport]
    public var reportsOmitted: Int64
    public var signoffs: [IntegrationSignoff]
    public var signoffsOmitted: Int64
    public var impact: IntegrationImpact?
    public var review: IntegrationReview?
    public var verify: IntegrationVerify?

    public init(
        recorded: Bool, impactError: String? = nil, plans: [IntegrationPlan] = [], plansOmitted: Int64 = 0,
        reports: [IntegrationReport] = [], reportsOmitted: Int64 = 0, signoffs: [IntegrationSignoff] = [],
        signoffsOmitted: Int64 = 0, impact: IntegrationImpact? = nil, review: IntegrationReview? = nil,
        verify: IntegrationVerify? = nil
    ) {
        self.recorded = recorded
        self.impactError = impactError
        self.plans = plans
        self.plansOmitted = plansOmitted
        self.reports = reports
        self.reportsOmitted = reportsOmitted
        self.signoffs = signoffs
        self.signoffsOmitted = signoffsOmitted
        self.impact = impact
        self.review = review
        self.verify = verify
    }

    /// Reads the member leniently: a part of the wrong type is dropped, never the item. Nil when the value is not an
    /// object with a boolean `recorded`.
    init?(wire json: JSONValue) {
        guard case .object = json, let recorded = json.wireBool("recorded") else { return nil }
        guard recorded else {
            self.init(recorded: false)
            return
        }
        self.init(
            recorded: true, impactError: json.wireString("impactError"),
            plans: json.wireList("plans", { IntegrationPlan(wire: $0) }), plansOmitted: json.wireCount("plansOmitted"),
            reports: json.wireList("reports", { IntegrationReport(wire: $0) }),
            reportsOmitted: json.wireCount("reportsOmitted"),
            signoffs: json.wireList("signoffs", { IntegrationSignoff(wire: $0) }),
            signoffsOmitted: json.wireCount("signoffsOmitted"),
            impact: json.optional("impact").flatMap { IntegrationImpact(wire: $0) },
            review: json.optional("review").flatMap { IntegrationReview(wire: $0) },
            verify: json.optional("verify").flatMap { IntegrationVerify(wire: $0) })
    }
}

extension JSONValue {
    /// A string member; another type or a missing member reads as absent.
    fileprivate func wireString(_ key: String) -> String? {
        guard case .string(let text)? = optional(key) else { return nil }
        return text
    }

    fileprivate func wireInt(_ key: String) -> Int64? {
        guard case .int(let number)? = optional(key) else { return nil }
        return number
    }

    /// A count; absent or of another type reads as 0, and it is never negative.
    fileprivate func wireCount(_ key: String) -> Int64 {
        max(0, wireInt(key) ?? 0)
    }

    fileprivate func wireBool(_ key: String) -> Bool? {
        guard case .bool(let flag)? = optional(key) else { return nil }
        return flag
    }

    /// The entries of an array member that read; an entry that does not is dropped.
    fileprivate func wireList<T>(_ key: String, _ make: (JSONValue) -> T?) -> [T] {
        guard case .array(let elements)? = optional(key) else { return [] }
        return elements.compactMap(make)
    }

    /// A text member with the `textVerified` and `truncated` flags beside it. Nil when the text is missing or not a
    /// string. A missing `textVerified` reads as false, so text the host did not vouch for is never shown.
    fileprivate func flaggedText(_ key: String) -> UntrustedText? {
        guard let text = wireString(key) else { return nil }
        return UntrustedText(raw: text, verified: wireBool("textVerified") ?? false, truncated: wireBool("truncated") ?? false)
    }
}

public struct AgentItem: WireItem {
    public let key: String
    public let agentId: String
    public let kind: String
    public let role: String
    public let generation: Int64
    public let state: String
    public let paused: Bool
    public let task: UntrustedText?
    /// The branch of the agent's pane; nil without a pane or from a host that does not send it.
    public let branch: UntrustedText?
    public let requestId: String
    public let contentHash: String
    /// What the host sees the agent doing; kept as the host's text, so a value a newer host adds is not lost. Nil when
    /// absent or null. Never shown as is: it is host text.
    public let activity: String?
    /// The time the activity last changed, as the host wrote it.
    public let activitySince: String?
    /// The sequence of the observation; a lower one never replaces a higher one.
    public let activitySeq: Int64?

    public init(json: JSONValue) throws {
        _ = try json.members("agent item")
        key = try json.string("key")
        agentId = try json.string("agentId")
        kind = try json.string("kind")
        role = try json.string("role")
        generation = try json.int64("generation")
        state = try json.string("state")
        paused = try json.bool("paused")
        task = try json.optionalString("task").map(itemText)
        branch = try json.optionalString("branch").map(itemText)
        requestId = try json.string("requestId")
        contentHash = try json.string("contentHash")
        activity = json.activityString("activity")
        activitySince = json.activityString("activitySince")
        activitySeq = json.activitySeq()
    }
}

extension JSONValue {
    /// A string member that may be missing, null or of another type; the last two read as absent.
    func activityString(_ key: String) -> String? {
        guard case .string(let text)? = optional(key) else { return nil }
        return text
    }

    func activitySeq() -> Int64? {
        guard case .int(let number)? = optional("activitySeq") else { return nil }
        return number
    }
}

public struct PackageCounts: Sendable, Equatable {
    public let total: Int64
    public let assigned: Int64
    public let reported: Int64
    public let integrated: Int64
}

public enum PackageState: Sendable, Equatable {
    case unassigned, assigned, reported, integrated, confirmed, cancelled
    case other(String)

    public init(wire: String) {
        switch wire {
        case "unassigned": self = .unassigned
        case "assigned": self = .assigned
        case "reported": self = .reported
        case "integrated": self = .integrated
        case "confirmed": self = .confirmed
        case "cancelled": self = .cancelled
        default: self = .other(wire)
        }
    }
}

/// One package of a plan as the host lists it (`packageDetails` of a plan item).
public struct PackageDetail: Codable, Sendable, Equatable {
    public var id: String
    public var title: UntrustedText
    public var role: String
    public var assigneeAgentId: String?
    public var state: PackageState
    public var dependsOn: [String]
    /// `passed`, `findings` or nil (not reviewed).
    public var review: String?
    public var integrationId: String?
    public var integrationState: String?

    public init(
        id: String, title: UntrustedText, role: String, assigneeAgentId: String? = nil, state: PackageState,
        dependsOn: [String] = [], review: String? = nil, integrationId: String? = nil, integrationState: String? = nil
    ) {
        self.id = id
        self.title = title
        self.role = role
        self.assigneeAgentId = assigneeAgentId
        self.state = state
        self.dependsOn = dependsOn
        self.review = review
        self.integrationId = integrationId
        self.integrationState = integrationState
    }

    init(json: JSONValue) throws {
        _ = try json.members("package detail")
        id = try json.string("id")
        title = itemText(try json.string("title"))
        role = try json.string("role")
        assigneeAgentId = try json.optionalString("assigneeAgentId")
        state = PackageState(wire: try json.string("state"))
        guard case .array(let names)? = json.optional("dependsOn") else {
            throw LineDecodeError.malformed("dependsOn is not an array")
        }
        dependsOn = try names.map { (name: JSONValue) -> String in
            guard case .string(let text) = name else { throw LineDecodeError.malformed("dependsOn entry is not a string") }
            return text
        }
        review = try json.optionalString("review")
        integrationId = try json.optionalString("integrationId")
        integrationState = try json.optionalString("integrationState")
    }
}

public struct PlanItem: WireItem {
    public let key: String
    public let planId: String
    public let sequence: Int64
    public let title: UntrustedText
    public let state: String
    public let packages: PackageCounts
    /// The packages in the host's order; nil from a host that does not send them.
    public let packageDetails: [PackageDetail]?
    /// Packages the host left out of `packageDetails`; nil from an older host.
    public let packageDetailsOmitted: Int64?

    public init(json: JSONValue) throws {
        _ = try json.members("plan item")
        key = try json.string("key")
        planId = try json.string("planId")
        sequence = try json.int64("sequence")
        title = itemText(try json.string("title"))
        state = try json.string("state")
        let counts = try json.required("packages")
        _ = try counts.members("packages")
        packages = PackageCounts(
            total: try counts.int64("total"),
            assigned: try counts.int64("assigned"),
            reported: try counts.int64("reported"),
            integrated: try counts.int64("integrated"))
        if let list = json.optional("packageDetails") {
            guard case .array(let elements) = list else { throw LineDecodeError.malformed("packageDetails is not an array") }
            packageDetails = try elements.map { try PackageDetail(json: $0) }
        } else {
            packageDetails = nil
        }
        packageDetailsOmitted = try json.optionalInt64("packageDetailsOmitted")
    }
}

public struct RunItem: WireItem {
    public let requestId: String
    public let state: String
    public let paused: Bool
    public let contentHash: String

    public init(json: JSONValue) throws {
        _ = try json.members("run item")
        requestId = try json.string("requestId")
        state = try json.string("state")
        paused = try json.bool("paused")
        contentHash = try json.string("contentHash")
    }
}

/// One page of a snapshot section.
public struct Page<T: WireItem>: Sendable, Equatable {
    public let head: Int64
    public let items: [T]
    public let next: String?

    public init(json: JSONValue) throws {
        _ = try json.members("page")
        head = try json.int64("head")
        guard case .array(let elements) = try json.required("items") else {
            throw LineDecodeError.malformed("items is not an array")
        }
        items = try elements.map { try T(json: $0) }
        next = try json.optionalString("next")
    }

    public init(data: Data) throws {
        try self.init(json: try decodeJSON(data))
    }
}

/// An item of the snapshot, by section.
public enum SnapshotItem: Sendable, Equatable {
    case waiting(WaitingItem)
    case agent(AgentItem)
    case plan(PlanItem)
    case run(RunItem)
    case unknown(section: String, JSONValue)

    public init(section: String, json: JSONValue) throws {
        switch section {
        case "waiting": self = .waiting(try WaitingItem(json: json))
        case "agents": self = .agent(try AgentItem(json: json))
        case "plans": self = .plan(try PlanItem(json: json))
        case "run": self = .run(try RunItem(json: json))
        default: self = .unknown(section: section, json)
        }
    }
}

/// The answer of `feed hash`.
public struct HashAnswer: Sendable, Equatable {
    public let requestId: String
    public let contentHash: String?

    public init(json: JSONValue) throws {
        _ = try json.members("hash answer")
        requestId = try json.string("requestId")
        contentHash = try json.optionalString("contentHash")
    }
}

/// The answer of `feed item`; `item` is nil when the row is gone and the client removes it.
public struct ItemAnswer: Sendable, Equatable {
    public let section: String
    public let key: String
    public let item: SnapshotItem?

    public init(json: JSONValue) throws {
        _ = try json.members("item answer")
        section = try json.string("section")
        key = try json.string("key")
        if let value = json.optional("item") {
            item = try SnapshotItem(section: section, json: value)
        } else {
            item = nil
        }
    }
}

/// Parses one JSON document; any failure is `LineDecodeError.malformed`.
func decodeJSON(_ data: Data) throws -> JSONValue {
    do {
        return try JSONDecoder().decode(JSONValue.self, from: data)
    } catch {
        throw LineDecodeError.malformed("not valid JSON")
    }
}
