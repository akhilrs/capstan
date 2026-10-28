import Foundation

/// The refusal codes of `cstan decide` (kernel, route and command line: twelve in all).
public enum DecideErrorCode: Sendable, Equatable {
    case notWaiting, hashMismatch, nonceReused, needsTerminal, noActivePm, limitReached, invalidRequest
    case forbidden, unauthorized
    case projectNotFound, noOperatorKey, notAnswering
    case other(String)

    /// The wire names of the known codes.
    public static let wireNames = [
        "not_waiting", "hash_mismatch", "nonce_reused", "needs_terminal", "no_active_pm", "limit_reached",
        "invalid_request", "forbidden", "unauthorized", "project_not_found", "no_operator_key", "not_answering",
    ]

    public init(wire: String) {
        switch wire {
        case "not_waiting": self = .notWaiting
        case "hash_mismatch": self = .hashMismatch
        case "nonce_reused": self = .nonceReused
        case "needs_terminal": self = .needsTerminal
        case "no_active_pm": self = .noActivePm
        case "limit_reached": self = .limitReached
        case "invalid_request": self = .invalidRequest
        case "forbidden": self = .forbidden
        case "unauthorized": self = .unauthorized
        case "project_not_found": self = .projectNotFound
        case "no_operator_key": self = .noOperatorKey
        case "not_answering": self = .notAnswering
        default: self = .other(wire)
        }
    }
}

/// A refusal of the host (or of this client before sending). The message is host text.
public struct DecideRefusal: Sendable, Equatable {
    public let code: DecideErrorCode
    public let message: UntrustedText

    public init(code: DecideErrorCode, message: UntrustedText) {
        self.code = code
        self.message = message
    }
}

public enum DecideKind: Sendable, Equatable, Codable {
    case applied, forwarded
}

/// The result of one decision.
public struct DecideResult: Sendable, Equatable {
    public let decisionId: String
    public let requestId: String
    public let nonce: String
    public let outcome: DecideKind
    /// True when the host had already handled this nonce and only returned the stored result.
    public let isRepeat: Bool
    public let kind: String
    public let verb: String
    public let contentHash: String
    public let eventSeq: Int64?
    public let messageId: String?
    /// The message of an earlier forward that this one replaces.
    public let replaces: String?

    public init(
        decisionId: String, requestId: String, nonce: String, outcome: DecideKind, isRepeat: Bool, kind: String,
        verb: String, contentHash: String, eventSeq: Int64? = nil, messageId: String? = nil, replaces: String? = nil
    ) {
        self.decisionId = decisionId
        self.requestId = requestId
        self.nonce = nonce
        self.outcome = outcome
        self.isRepeat = isRepeat
        self.kind = kind
        self.verb = verb
        self.contentHash = contentHash
        self.eventSeq = eventSeq
        self.messageId = messageId
        self.replaces = replaces
    }

    public init(json: JSONValue) throws {
        _ = try json.members("decide result")
        guard try json.int64("v") == 1 else { throw LineDecodeError.unsupportedVersion(Int(try json.int64("v"))) }
        decisionId = try json.string("decisionId")
        requestId = try json.string("requestId")
        nonce = try json.string("nonce")
        switch try json.string("outcome") {
        case "applied": outcome = .applied
        case "forwarded": outcome = .forwarded
        default: throw LineDecodeError.malformed("unknown outcome")
        }
        isRepeat = try json.bool("repeat")
        kind = try json.string("kind")
        verb = try json.string("verb")
        contentHash = try json.string("contentHash")
        eventSeq = try json.optionalInt64("eventSeq")
        messageId = try json.optionalString("messageId")
        replaces = try json.optionalString("replaces")
    }
}

/// One line of `cstan decide --json`: a result, or `{v: 1, error: {code, message}}`.
public enum DecideAnswer: Sendable, Equatable {
    case result(DecideResult)
    case refusal(DecideRefusal)

    public init(json: JSONValue) throws {
        _ = try json.members("decide answer")
        if let error = json.optional("error") {
            guard try json.int64("v") == 1 else { throw LineDecodeError.unsupportedVersion(Int(try json.int64("v"))) }
            _ = try error.members("error")
            let message = try error.optionalString("message") ?? ""
            self = .refusal(DecideRefusal(
                code: DecideErrorCode(wire: try error.string("code")),
                message: UntrustedText(raw: message, verified: true, truncated: false)))
        } else {
            self = .result(try DecideResult(json: json))
        }
    }

    public init(data: Data) throws {
        try self.init(json: try decodeJSON(data))
    }
}

/// A row of the verbs table of the protocol document.
public struct DecideVerbRow: Sendable, Equatable {
    public let kind: String
    public let verb: String
    public let outcome: String

    public init(kind: String, verb: String, outcome: String) {
        self.kind = kind
        self.verb = verb
        self.outcome = outcome
    }

    /// The rows the host accepts: its decide grammar, one row per kind and verb.
    public static let documented: [DecideVerbRow] = [
        DecideVerbRow(kind: "op", verb: "deny", outcome: "applied"),
        DecideVerbRow(kind: "op", verb: "approve", outcome: "forwarded"),
        DecideVerbRow(kind: "prompt", verb: "option", outcome: "forwarded"),
        DecideVerbRow(kind: "prompt", verb: "esc", outcome: "forwarded"),
        DecideVerbRow(kind: "design", verb: "approve", outcome: "applied"),
        DecideVerbRow(kind: "design", verb: "changes", outcome: "applied"),
        DecideVerbRow(kind: "message", verb: "reply", outcome: "applied"),
        DecideVerbRow(kind: "agent", verb: "pause", outcome: "applied"),
        DecideVerbRow(kind: "agent", verb: "resume", outcome: "applied"),
        DecideVerbRow(kind: "run", verb: "pause", outcome: "applied"),
        DecideVerbRow(kind: "run", verb: "resume", outcome: "applied"),
    ]
}
