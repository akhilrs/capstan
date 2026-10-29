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
    }

    static func validSha(_ text: String) -> String? {
        let hex = text.utf8.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
        return hex && (7...64).contains(text.utf8.count) ? text : nil
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
