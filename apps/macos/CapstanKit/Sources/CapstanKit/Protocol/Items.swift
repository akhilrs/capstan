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

    public init(json: JSONValue) throws {
        _ = try json.members("waiting item")
        key = try json.string("key")
        requestId = try json.string("requestId")
        kind = WaitingKind(wire: try json.string("kind"))
        since = try json.string("since")
        summary = itemText(try json.string("summary"))
        contentHash = try json.string("contentHash")
        needsTerminal = try json.bool("needsTerminal")
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

public struct PlanItem: WireItem {
    public let key: String
    public let planId: String
    public let sequence: Int64
    public let title: UntrustedText
    public let state: String
    public let packages: PackageCounts

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
