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
    /// The link of a design item, taken by the host from the designer's report; nil for another kind, when the report
    /// names none and from a host that does not send it. Untrusted text: shown as text, opened only when http or https.
    public let designLink: UntrustedText?
    /// What a client may offer for `op approve-as`. nil for another kind and from a host older than `approve-as`
    /// (absent or null), which is how a client knows to offer only `op approve`.
    public let approveChoices: ApproveChoices?

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
        designLink = try json.optionalString("designLink").flatMap { $0.isEmpty ? nil : itemText($0) }
        approveChoices = try json.optional("approveChoices").map(ApproveChoices.init(json:))
    }

    static func validSha(_ text: String) -> String? {
        let hex = text.utf8.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
        return hex && (7...64).contains(text.utf8.count) ? text : nil
    }
}

// MARK: Approve choices (document section 5, `approveChoices`)

/// What a client may offer for `op approve-as` on one Operator proposal.
public struct ApproveChoices: Codable, Sendable, Equatable {
    public enum AutoKind: Codable, Sendable, Equatable {
        case hour, session
        case other(String)

        public init(wire: String) {
            switch wire {
            case "hour": self = .hour
            case "session": self = .session
            default: self = .other(wire)
            }
        }

        public var wireName: String {
            switch self {
            case .hour: return "hour"
            case .session: return "session"
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

    /// Why only `once` is offered (or, for `textMismatch`, nothing but a deny).
    public enum Limit: Codable, Sendable, Equatable {
        case floor, restart, truncated, notChecked, textMismatch
        case other(String)

        public init(wire: String) {
            switch wire {
            case "floor": self = .floor
            case "restart": self = .restart
            case "truncated": self = .truncated
            case "not_checked": self = .notChecked
            case "text_mismatch": self = .textMismatch
            default: self = .other(wire)
            }
        }

        public var wireName: String {
            switch self {
            case .floor: return "floor"
            case .restart: return "restart"
            case .truncated: return "truncated"
            case .notChecked: return "not_checked"
            case .textMismatch: return "text_mismatch"
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

    public let once: Bool
    public let exact: Bool
    /// The offered prefixes, shortest first. The received string is kept: the call sends it byte for byte.
    public let prefixes: [UntrustedText]
    public let auto: [AutoKind]
    public let limit: Limit?
    /// The floor entry that holds the proposal back (`limit` floor); host text.
    public let floorEntry: String?

    public init(
        once: Bool, exact: Bool, prefixes: [UntrustedText] = [], auto: [AutoKind] = [], limit: Limit? = nil,
        floorEntry: String? = nil
    ) {
        self.once = once
        self.exact = exact
        self.prefixes = prefixes
        self.auto = auto
        self.limit = limit
        self.floorEntry = floorEntry
    }

    public init(json: JSONValue) throws {
        _ = try json.members("approve choices")
        once = try json.bool("once")
        exact = try json.bool("exact")
        guard case .array(let list) = try json.required("prefixes") else {
            throw LineDecodeError.malformed("prefixes is not an array")
        }
        prefixes = try list.map { value in
            guard case .string(let text) = value else { throw LineDecodeError.malformed("prefix is not a string") }
            return itemText(text)
        }
        guard case .array(let kinds) = try json.required("auto") else {
            throw LineDecodeError.malformed("auto is not an array")
        }
        auto = try kinds.map { value in
            guard case .string(let name) = value else { throw LineDecodeError.malformed("auto is not a string") }
            return AutoKind(wire: name)
        }
        limit = try json.optionalString("limit").map(Limit.init(wire:))
        floorEntry = try json.optionalString("floorEntry")
    }

    /// The wire form, so a stored item read back through `WaitingItem(json:)` keeps its choices.
    var json: JSONValue {
        var members: [String: JSONValue] = [
            "once": .bool(once), "exact": .bool(exact), "prefixes": .array(prefixes.map { JSONValue.string($0.raw) }),
            "auto": .array(auto.map { JSONValue.string($0.wireName) }),
        ]
        if let limit { members["limit"] = .string(limit.wireName) }
        if let floorEntry { members["floorEntry"] = .string(floorEntry) }
        return .object(members)
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
    /// The model the agent runs on and the layer that chose it (`modelSource`); nil when absent or null.
    public let model: UntrustedText?
    public let modelSource: String?
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
        model = json.activityString("model").map(itemText)
        modelSource = json.activityString("modelSource")
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
    /// `plan:<planId>`, the request a cancel is sent on; nil from a host that cannot cancel a plan remotely.
    public let requestId: String?
    /// The binding of that request; nil with `requestId`.
    public let contentHash: String?

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
        requestId = try json.optionalString("requestId")
        contentHash = try json.optionalString("contentHash")
    }
}

public struct RunItem: WireItem {
    public let requestId: String
    public let state: String
    public let paused: Bool
    public let contentHash: String
    /// The PM card; nil when the project has no active PM, and from a host that does not send it.
    public let pm: PmCard?

    public init(json: JSONValue) throws {
        _ = try json.members("run item")
        requestId = try json.string("requestId")
        state = try json.string("state")
        paused = try json.bool("paused")
        contentHash = try json.string("contentHash")
        pm = json.optional("pm").flatMap { PmCard(wire: $0) }
    }
}

// MARK: PM card (document section 5, `pm`)

/// The `pm` member of the run item: who the active PM is, what it waits on, the person's newest message to it and its
/// open stuck episode. Read leniently: a member that does not read is dropped, never the run item.
public struct PmCard: Codable, Sendable, Equatable {
    /// One kind of work the PM waits on, with a count above 0 and the oldest time.
    public struct Waiting: Codable, Sendable, Equatable {
        /// `report`, `review`, `verify`, `plan_review`, `user`, or a kind a newer host adds.
        public var kind: String
        public var count: Int64
        public var since: String?

        public init(kind: String, count: Int64, since: String?) {
            self.kind = kind
            self.count = count
            self.since = since
        }

        init?(wire json: JSONValue) {
            guard let kind = json.wireString("kind") else { return nil }
            let count = json.wireCount("count")
            guard count > 0 else { return nil }
            self.init(kind: kind, count: count, since: json.wireString("since"))
        }

        var json: JSONValue {
            var members: [String: JSONValue] = ["kind": .string(kind), "count": .int(count)]
            if let since { members["since"] = .string(since) }
            return .object(members)
        }
    }

    /// The person's newest message to the active PM.
    public struct FromUser: Codable, Sendable, Equatable {
        public var messageId: String
        public var client: String?
        public var createdAt: String
        public var state: String?
        public var stateSince: String?
        /// `queued`, `delivered`, `read`, `not_delivered`, or a value a newer host adds.
        public var delivery: String
        /// Nil when the host could not vouch for the stored text (`body` null).
        public var body: UntrustedText?

        public init(
            messageId: String, client: String? = nil, createdAt: String, state: String? = nil,
            stateSince: String? = nil, delivery: String, body: UntrustedText?
        ) {
            self.messageId = messageId
            self.client = client
            self.createdAt = createdAt
            self.state = state
            self.stateSince = stateSince
            self.delivery = delivery
            self.body = body
        }

        init?(wire json: JSONValue) {
            guard let messageId = json.wireString("messageId"), let createdAt = json.wireString("createdAt"),
                  let delivery = json.wireString("delivery") else { return nil }
            let body = json.optional("body").flatMap { $0.flaggedText("text") }
            self.init(
                messageId: messageId, client: json.wireString("client"), createdAt: createdAt,
                state: json.wireString("state"), stateSince: json.wireString("stateSince"), delivery: delivery,
                body: body.flatMap { $0.verified ? $0 : nil })
        }

        var json: JSONValue {
            var members: [String: JSONValue] = [
                "messageId": .string(messageId), "createdAt": .string(createdAt), "delivery": .string(delivery),
            ]
            if let client { members["client"] = .string(client) }
            if let state { members["state"] = .string(state) }
            if let stateSince { members["stateSince"] = .string(stateSince) }
            if let body {
                members["body"] = .object([
                    "text": .string(body.raw), "textVerified": .bool(body.verified), "truncated": .bool(body.truncated),
                ])
            } else {
                members["body"] = .null
            }
            return .object(members)
        }
    }

    /// The open stuck episode of the PM.
    public struct Stuck: Codable, Sendable, Equatable {
        public var episodeId: String
        /// `unread_mail`, `no_progress`, `prompt`, or a reason a newer host adds.
        public var reason: String
        public var since: String?
        /// False when an alerting episode of the same PM opened in the last 30 minutes: the banner shows, no alert.
        public var alert: Bool

        public init(episodeId: String, reason: String, since: String?, alert: Bool) {
            self.episodeId = episodeId
            self.reason = reason
            self.since = since
            self.alert = alert
        }

        init?(wire json: JSONValue) {
            guard let episodeId = json.wireString("episodeId"), let reason = json.wireString("reason") else { return nil }
            self.init(
                episodeId: episodeId, reason: reason, since: json.wireString("since"),
                alert: json.wireBool("alert") ?? false)
        }

        var json: JSONValue {
            var members: [String: JSONValue] = [
                "episodeId": .string(episodeId), "reason": .string(reason), "alert": .bool(alert),
            ]
            if let since { members["since"] = .string(since) }
            return .object(members)
        }
    }

    public var agentId: String
    public var waitingOn: [Waiting]
    public var fromUser: FromUser?
    public var stuck: Stuck?

    public init(agentId: String, waitingOn: [Waiting] = [], fromUser: FromUser? = nil, stuck: Stuck? = nil) {
        self.agentId = agentId
        self.waitingOn = waitingOn
        self.fromUser = fromUser
        self.stuck = stuck
    }

    /// Nil when the value is not an object or has no agent id.
    public init?(wire json: JSONValue) {
        guard let agentId = json.wireString("agentId"), !agentId.isEmpty else { return nil }
        self.init(
            agentId: agentId, waitingOn: json.wireList("waitingOn") { Waiting(wire: $0) },
            fromUser: json.optional("fromUser").flatMap { FromUser(wire: $0) },
            stuck: json.optional("stuck").flatMap { Stuck(wire: $0) })
    }

    /// The card as the host sends it, so an effect merged over the stored run keeps it.
    public var json: JSONValue {
        .object([
            "agentId": .string(agentId),
            "waitingOn": .array(waitingOn.map(\.json)),
            "fromUser": fromUser?.json ?? .null,
            "stuck": stuck?.json ?? .null,
        ])
    }
}

/// A message from an agent to the person (`cstan send @user`), as the `messages` section lists it. The body is the
/// agent's text: shown as plain text, never as a link or rich text, and never in a notification.
public struct MessageItem: WireItem {
    public let key: String
    public let messageId: String
    public let sequence: Int64
    public let senderAgentId: String
    /// `action` (a question) or `info`.
    public let messageClass: String
    /// `open` (a question not answered yet), `answered` or `info`.
    public let state: String
    public let body: UntrustedText
    public let createdAt: String
    public let answeredAt: String?
    /// `message:<messageId>`: the request id of the waiting item while the question is open.
    public let requestId: String
    /// The choices of a question, in order; empty without, and from a host older than them.
    public let options: [String]

    public init(json: JSONValue) throws {
        _ = try json.members("message item")
        key = try json.string("key")
        messageId = try json.string("messageId")
        sequence = try json.int64("sequence")
        senderAgentId = try json.string("senderAgentId")
        messageClass = try json.string("class")
        state = try json.string("state")
        body = UntrustedText(raw: try json.string("body"), verified: true, truncated: try json.bool("truncated"))
        createdAt = try json.string("createdAt")
        answeredAt = try json.optionalString("answeredAt")
        requestId = try json.string("requestId")
        if case .array(let list)? = json.optional("options") {
            options = list.compactMap { value -> String? in
                if case .string(let text) = value { return text }
                return nil
            }
        } else {
            options = []
        }
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
    case message(MessageItem)
    case unknown(section: String, JSONValue)

    public init(section: String, json: JSONValue) throws {
        switch section {
        case "waiting": self = .waiting(try WaitingItem(json: json))
        case "agents": self = .agent(try AgentItem(json: json))
        case "plans": self = .plan(try PlanItem(json: json))
        case "run": self = .run(try RunItem(json: json))
        case "messages": self = .message(try MessageItem(json: json))
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
