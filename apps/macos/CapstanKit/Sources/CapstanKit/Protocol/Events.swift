import Foundation

public enum FeedEventType {
    /// Every event type named in document section 5.
    public static let known: [String] = [
        "message.action_needed", "message.state",
        "prompt.waiting", "prompt.answered", "prompt.expired",
        "operator.proposed", "operator.decided", "operator.finished",
        "report.recorded", "design.decided",
        "plan.submitted", "plan.approved", "plan.signed_off", "plan.changed", "plan.needs_attention",
        "review.verdict", "verify.result",
        "finding.raised", "finding.resolved",
        "agent.registered", "agent.lost", "agent.paused", "agent.resumed", "agent.ended", "agent.blocked",
        "run.changed", "integration.finished", "snapshot.invalidated",
    ]
}

public enum EffectOp: Sendable, Equatable {
    case upsert, remove, invalidate
    case other(String)

    public init(wire: String) {
        switch wire {
        case "upsert": self = .upsert
        case "remove": self = .remove
        case "invalidate": self = .invalidate
        default: self = .other(wire)
        }
    }
}

public struct Effect: Sendable, Equatable {
    public let section: String
    public let op: EffectOp
    public let key: String
    public let fields: JSONValue
}

public struct EntityRef: Sendable, Equatable {
    public let type: String
    public let id: String
}

public struct PromptOption: Sendable, Equatable {
    public let number: Int64
    public let text: UntrustedText
    public let acceptsText: Bool
    public let widensPermissions: Bool
}

public struct OperatorBody: Sendable, Equatable {
    public let proposalId: String
    public let kind: String
    public let state: String
    public let command: UntrustedText
    public let decision: String?
    public let reason: UntrustedText
}

public struct PromptBody: Sendable, Equatable {
    public let relayId: String
    public let agentId: String
    public let state: String
    public let text: UntrustedText
    public let options: [PromptOption]
    public let reason: UntrustedText
}

public struct MessageBody: Sendable, Equatable {
    public let messageId: String
    public let recipientAgentId: String
    public let state: String
    public let body: UntrustedText
    public let reason: UntrustedText
}

public struct AgentBody: Sendable, Equatable {
    public let agentId: String
    public let kind: String
    public let role: String
    public let generation: Int64?
    public let paused: Bool?
    public let state: String
    public let blocker: String?
    public let scope: String?
    /// Nil when the event type carries no reason.
    public let reason: UntrustedText?
}

public struct RunBody: Sendable, Equatable {
    public let state: String
    public let paused: Bool
    public let reason: UntrustedText
}

public struct DesignBody: Sendable, Equatable {
    public let planId: String
    public let packageId: String
    public let reportId: String
    public let decision: String
    public let link: UntrustedText
}

public struct EventBody: Sendable, Equatable {
    public let type: String
    public let effects: [Effect]
    /// Values that can move after the event; nil when the read had none.
    public let current: JSONValue?
    /// The whole body as the host sent it.
    public let raw: JSONValue

    init(type: String, raw: JSONValue) throws {
        _ = try raw.members("body")
        guard case .array(let list) = try raw.required("effects") else {
            throw LineDecodeError.malformed("effects is not an array")
        }
        self.type = type
        self.raw = raw
        self.current = raw.optional("current")
        self.effects = try list.map { item in
            _ = try item.members("effect")
            return Effect(
                section: try item.string("section"),
                op: EffectOp(wire: try item.string("op")),
                key: try item.string("key"),
                fields: item["fields"] ?? .null)
        }
    }

    private var textVerified: Bool {
        if case .bool(let flag)? = raw["textVerified"] { return flag }
        return true
    }

    private var truncated: Bool {
        if case .bool(let flag)? = raw["truncated"] { return flag }
        return false
    }

    private func text(_ key: String) -> UntrustedText {
        var value = ""
        if case .string(let text)? = raw[key] { value = text }
        return UntrustedText(raw: value, verified: textVerified, truncated: truncated)
    }

    private func has(prefix: String) -> Bool { type.hasPrefix(prefix) }

    public var operatorBody: OperatorBody? {
        guard has(prefix: "operator."), let proposalId = try? raw.string("proposalId"),
              let kind = try? raw.string("kind"), let state = try? raw.string("state") else { return nil }
        return OperatorBody(
            proposalId: proposalId, kind: kind, state: state, command: text("command"),
            decision: try? raw.optionalString("decision"), reason: text("reason"))
    }

    public var promptBody: PromptBody? {
        guard has(prefix: "prompt."), let relayId = try? raw.string("relayId"),
              let agentId = try? raw.string("agentId"), let state = try? raw.string("state") else { return nil }
        var options: [PromptOption] = []
        if textVerified, case .array(let list)? = raw["options"] {
            for item in list.prefix(ProtocolLimits.optionsCap) {
                guard let number = try? item.int64("number"), let label = try? item.string("text"),
                      let accepts = try? item.bool("acceptsText"),
                      let widens = try? item.bool("widensPermissions") else { continue }
                options.append(PromptOption(
                    number: number,
                    text: UntrustedText(raw: label, verified: true, truncated: truncated),
                    acceptsText: accepts, widensPermissions: widens))
            }
        }
        return PromptBody(
            relayId: relayId, agentId: agentId, state: state, text: text("text"),
            options: options, reason: text("reason"))
    }

    public var messageBody: MessageBody? {
        guard has(prefix: "message."), let messageId = try? raw.string("messageId"),
              let recipient = try? raw.string("recipientAgentId"), let state = try? raw.string("state") else { return nil }
        return MessageBody(
            messageId: messageId, recipientAgentId: recipient, state: state,
            body: text("body"), reason: text("reason"))
    }

    public var agentBody: AgentBody? {
        guard has(prefix: "agent."), let agentId = try? raw.string("agentId"),
              let kind = try? raw.string("kind"), let role = try? raw.string("role"),
              let state = try? raw.string("state") else { return nil }
        return AgentBody(
            agentId: agentId, kind: kind, role: role,
            generation: try? raw.optionalInt64("generation"), paused: try? raw.optionalBool("paused"),
            state: state, blocker: try? raw.optionalString("blocker"), scope: try? raw.optionalString("scope"),
            reason: raw["reason"] == nil ? nil : text("reason"))
    }

    public var runBody: RunBody? {
        guard type == "run.changed", let state = try? raw.string("state"), let paused = try? raw.bool("paused") else { return nil }
        return RunBody(state: state, paused: paused, reason: text("reason"))
    }

    public var designBody: DesignBody? {
        guard type == "design.decided", let planId = try? raw.string("planId"),
              let packageId = try? raw.string("packageId"), let reportId = try? raw.string("reportId"),
              let decision = try? raw.string("decision"), let link = try? raw.string("link") else { return nil }
        return DesignBody(
            planId: planId, packageId: packageId, reportId: reportId, decision: decision,
            link: UntrustedText(raw: link, verified: true, truncated: false))
    }
}

public struct FeedEvent: Sendable, Equatable, Decodable {
    public let id: String
    public let seq: Int64
    public let k: Int64
    public let ts: String
    public let type: String
    public let actorId: String?
    public let entity: EntityRef
    public let from: String?
    public let to: String?
    public let body: EventBody

    public init(json: JSONValue) throws {
        _ = try json.members("event")
        id = try json.string("id")
        seq = try json.int64("seq")
        k = try json.int64("k")
        ts = try json.string("ts")
        type = try json.string("type")
        actorId = try json.optionalString("actorId")
        let entityValue = try json.required("entity")
        _ = try entityValue.members("entity")
        entity = EntityRef(type: try entityValue.string("type"), id: try entityValue.string("id"))
        from = try json.optionalString("from")
        to = try json.optionalString("to")
        body = try EventBody(type: type, raw: try json.required("body"))
    }

    public init(from decoder: Decoder) throws {
        let value = try JSONValue(from: decoder)
        do {
            try self.init(json: value)
        } catch let error as LineDecodeError {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "\(error)"))
        }
    }
}
