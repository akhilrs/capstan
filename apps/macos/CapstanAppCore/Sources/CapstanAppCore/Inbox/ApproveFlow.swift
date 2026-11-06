import CapstanKit
import Foundation

/// The steps of the approve sheet for an Operator proposal that carries `approveChoices`.
public enum ApproveStep: Sendable, Equatable {
    case answer, scope, window
}

/// The answer chosen on step 1.
public enum ApproveAnswer: Sendable, Equatable {
    case once
    /// Approve and allow similar commands for the session; step 2 picks the exact command or a prefix.
    case similar
    /// Approve and open an auto-approve window; step 2 picks its length.
    case auto
}

/// What step 2 of "similar" grants: the exact command or one offered prefix, as received.
public enum ApproveScope: Sendable, Equatable {
    case exact
    case prefix(String)
}

/// What the primary button of the sheet does.
public enum ApproveAction: Sendable, Equatable {
    /// Nothing: the primary is disabled.
    case disabled
    /// The step changed.
    case moved
    /// Ask before opening the auto-approve window.
    case confirmAuto
    /// Send this decision.
    case send(DecideVerb)
}

/// The state of the approve sheet. Pure: the view holds one and shows what it says. The sheet is offered by
/// `ApproveFlow(choices:commandVerified:)`; a proposal that is held back to Deny only gets no flow.
public struct ApproveFlow: Sendable, Equatable {
    public let choices: ApproveChoices
    public private(set) var step: ApproveStep = .answer
    public private(set) var answer: ApproveAnswer?
    public private(set) var scope: ApproveScope?
    public private(set) var window: ApproveChoices.AutoKind?

    /// nil when the sheet cannot be offered: the command is not shown whole and checked, it is too long to show
    /// (`truncated`), it does not match its hash (`text_mismatch`), or the host offers nothing.
    public init?(choices: ApproveChoices, commandVerified: Bool) {
        guard commandVerified, choices.once else { return nil }
        if choices.limit == .truncated || choices.limit == .textMismatch { return nil }
        self.choices = choices
    }

    // MARK: What the host offers

    /// A held-back proposal is offered `once` only.
    public var isLimited: Bool { choices.limit != nil }

    /// The prefixes to show: an empty one is never offered and a repeated one (same bytes) is shown once. Order as received (shortest first).
    public var prefixes: [String] {
        guard !isLimited else { return [] }
        var seen = Set<[UInt8]>()
        return choices.prefixes.map(\.raw).filter { !$0.isEmpty && seen.insert(Array($0.utf8)).inserted }
    }

    public var offersExact: Bool { !isLimited && choices.exact }

    public var windows: [ApproveChoices.AutoKind] {
        isLimited ? [] : [.hour, .session].filter { choices.auto.contains($0) }
    }

    /// The answers of step 1, in order.
    public var answers: [ApproveAnswer] {
        var list: [ApproveAnswer] = [.once]
        if offersExact || !prefixes.isEmpty { list.append(.similar) }
        if !windows.isEmpty { list.append(.auto) }
        return list
    }

    // MARK: Choosing

    public mutating func choose(_ answer: ApproveAnswer) {
        guard step == .answer, answers.contains(answer) else { return }
        self.answer = answer
        scope = nil
        window = nil
    }

    public mutating func choose(scope: ApproveScope) {
        guard step == .scope else { return }
        switch scope {
        case .exact: if offersExact { self.scope = scope }
        case .prefix(let raw): if prefixes.contains(where: { Array($0.utf8) == Array(raw.utf8) }) { self.scope = scope }
        }
    }

    public mutating func choose(window: ApproveChoices.AutoKind) {
        guard step == .window, windows.contains(window) else { return }
        self.window = window
    }

    public mutating func back() {
        guard step != .answer else { return }
        step = .answer
        scope = nil
        window = nil
    }

    // MARK: The primary button

    /// "1 of 2" while a two step answer is chosen; nothing for approve once or before a choice.
    public var stepText: String? {
        switch (step, answer) {
        case (.answer, .similar?), (.answer, .auto?): return count(1)
        case (.scope, _), (.window, _): return count(2)
        default: return nil
        }
    }

    private func count(_ n: Int) -> String {
        ApproveStrings.stepOf.replacingOccurrences(of: "{n}", with: String(n))
            .replacingOccurrences(of: "{total}", with: "2")
    }

    /// The words on the primary button; they name the result.
    public var primaryTitle: String {
        switch step {
        case .answer:
            switch answer {
            case nil: return ApproveStrings.choicesNone
            case .once?: return ApproveStrings.onceConfirmAction
            case .similar?, .auto?: return ApproveStrings.autoNext
            }
        case .scope: return ApproveStrings.scopeAction
        case .window: return ApproveStrings.autoNext
        }
    }

    public var canAdvance: Bool {
        switch step {
        case .answer: return answer != nil
        case .scope: return scope != nil
        case .window: return window != nil
        }
    }

    /// The primary button was pressed.
    public mutating func advance() -> ApproveAction {
        guard canAdvance else { return .disabled }
        switch step {
        case .answer:
            switch answer {
            case .once?: return .send(.opApproveAs(.once))
            case .similar?: step = .scope; return .moved
            case .auto?: step = .window; return .moved
            case nil: return .disabled
            }
        case .scope:
            switch scope {
            case .exact?: return .send(.opApproveAs(.exact))
            case .prefix(let raw)?: return .send(.opApproveAs(.prefix(raw: raw)))
            case nil: return .disabled
            }
        case .window:
            return .confirmAuto
        }
    }

    /// The decision to send once the person confirmed the window.
    public func confirmedWindow() -> DecideVerb? {
        guard step == .window, let window else { return nil }
        switch window {
        case .hour: return .opApproveAs(.autoHour)
        case .session: return .opApproveAs(.autoSession)
        case .other: return nil
        }
    }
}

/// The words for a proposal that the host holds back, shown under the command.
public struct ApproveLimit: Sendable, Equatable {
    /// "Only Approve once and Deny are offered."; nil when Deny is the only thing left.
    public let lead: String?
    public let reason: String
    /// The floor entry named by the host (host text; show it as untrusted).
    public let floorEntry: String?

    /// nil when the host holds nothing back.
    public static func make(_ choices: ApproveChoices) -> ApproveLimit? {
        guard let limit = choices.limit else { return nil }
        switch limit {
        case .truncated: return ApproveLimit(lead: nil, reason: ApproveStrings.limitedTruncated, floorEntry: nil)
        case .textMismatch: return ApproveLimit(lead: nil, reason: ApproveStrings.limitedTextMismatch, floorEntry: nil)
        case .floor:
            return ApproveLimit(
                lead: ApproveStrings.limitedLead, reason: ApproveStrings.limitedFloor, floorEntry: choices.floorEntry)
        case .restart: return ApproveLimit(lead: ApproveStrings.limitedLead, reason: ApproveStrings.limitedRestart, floorEntry: nil)
        case .notChecked:
            return ApproveLimit(lead: ApproveStrings.limitedLead, reason: ApproveStrings.limitedNotChecked, floorEntry: nil)
        case .other: return ApproveLimit(lead: ApproveStrings.limitedLead, reason: "", floorEntry: nil)
        }
    }
}
