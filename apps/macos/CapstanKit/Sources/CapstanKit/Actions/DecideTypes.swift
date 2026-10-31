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
    /// What a merge made (`integration merge` only).
    public let merge: MergeResult?

    public init(
        decisionId: String, requestId: String, nonce: String, outcome: DecideKind, isRepeat: Bool, kind: String,
        verb: String, contentHash: String, eventSeq: Int64? = nil, messageId: String? = nil, replaces: String? = nil,
        merge: MergeResult? = nil
    ) {
        self.merge = merge
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
        merge = try json.optional("merge").map(MergeResult.init(json:))
    }
}

/// What `integration merge` made (section 11, Merge). Ids and states only: no host text.
public struct MergeResult: Sendable, Equatable {
    public enum Verify: Sendable, Equatable {
        case passed(runId: String)
        case notConfigured
        case other(String)
    }

    public let integrationId: String
    public let branch: String
    /// nil when the head was already in the project's branch (`alreadyInHead`).
    public let mergeCommit: String?
    public let alreadyInHead: Bool
    public let verify: Verify
    /// The merged branch is cleaned up after the merge; false when cleanup is off.
    public let cleanupScheduled: Bool
    /// Always true: the merge never pushes.
    public let pushDisabled: Bool
    /// The branch merged into; nil from a host older than `--into`.
    public var into: String? = nil

    public init(
        integrationId: String, branch: String, mergeCommit: String?, alreadyInHead: Bool, verify: Verify,
        cleanupScheduled: Bool, pushDisabled: Bool = true
    ) {
        self.integrationId = integrationId
        self.branch = branch
        self.mergeCommit = mergeCommit
        self.alreadyInHead = alreadyInHead
        self.verify = verify
        self.cleanupScheduled = cleanupScheduled
        self.pushDisabled = pushDisabled
    }

    public init(json: JSONValue) throws {
        _ = try json.members("merge")
        integrationId = try json.string("integrationId")
        branch = try json.string("branch")
        mergeCommit = try json.optionalString("mergeCommit")
        alreadyInHead = try json.bool("alreadyInHead")
        into = try? json.optionalString("into")
        let verifyValue = try json.required("verify")
        switch try verifyValue.string("state") {
        case "passed": verify = .passed(runId: try verifyValue.optionalString("runId") ?? "")
        case "not_configured": verify = .notConfigured
        case let state: verify = .other(state)
        }
        if let cleanup = json.optional("cleanup") {
            cleanupScheduled = (try? cleanup.string("state")) == "scheduled"
        } else {
            cleanupScheduled = false
        }
        if let push = json.optional("push") {
            pushDisabled = (try? push.string("state")) != "enabled"
        } else {
            pushDisabled = true
        }
    }

    /// The refusal codes of the merge, in the order its gates answer (they decode as `DecideErrorCode.other`).
    public static let refusalCodes = [
        "merge_unrecorded", "merge_disabled", "invalid_commit", "invalid_branch", "no_identity", "run_paused", "busy",
        "not_reviewed", "not_verified", "branch_moved", "unsafe_config", "wrong_branch", "dirty_tree", "merge_conflict",
        "git_too_old", "merge_failed", "git_unavailable",
    ]
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
        DecideVerbRow(kind: "integration", verb: "merge", outcome: "applied"),
        DecideVerbRow(kind: "pm", verb: "message", outcome: "applied"),
    ]
}
