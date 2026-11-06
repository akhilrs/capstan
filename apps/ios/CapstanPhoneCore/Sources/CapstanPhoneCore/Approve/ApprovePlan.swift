import CapstanKit
import Foundation

/// What the details screen offers for an Operator proposal, worked out from what the host listed in `approveChoices`.
/// No view logic and no host text beyond the strings it carries unchanged.
public enum ApproveOffer: Equatable, Sendable {
    /// The host sent no `approveChoices` (an old host): the approval goes to the PM as before.
    case oldHost
    /// The host lets this phone approve as the operator.
    case sheet(ApprovePlan)
    /// Nothing may be approved from here; Deny stays. `limit` says why (`text_mismatch`, `truncated`, or no choice).
    case denyOnly(ApproveChoices.Limit?)

    /// - Parameters:
    ///   - choices: `StoredWaiting.approveChoices`.
    ///   - commandVerified: the command passed its integrity check.
    ///   - commandWhole: the command is shown whole (not cut by the host or the app).
    public static func make(_ choices: ApproveChoices?, commandVerified: Bool, commandWhole: Bool) -> ApproveOffer {
        guard let choices else { return .oldHost }
        guard commandVerified, commandWhole else { return .denyOnly(choices.limit) }
        switch choices.limit {
        case .textMismatch?, .truncated?: return .denyOnly(choices.limit)
        default: break
        }
        let plan = ApprovePlan(choices)
        return plan.offersAnything ? .sheet(plan) : .denyOnly(choices.limit)
    }
}

/// The top-level answers of step 1.
public enum ApproveTop: Equatable, Sendable, CaseIterable {
    case once, similar, auto
}

/// One row of the scope step: the exact command, or one offered prefix.
public enum ApproveScope: Equatable, Sendable {
    case exact
    /// `raw` is the offered prefix byte for byte; it is shown and sent as it is, never trimmed, split or joined.
    case prefix(raw: String)

    public var choice: ApproveChoice {
        switch self {
        case .exact: return .exact
        case .prefix(let raw): return .prefix(raw: raw)
        }
    }
}

/// The window lengths of the auto step.
public enum ApproveWindow: Equatable, Sendable {
    case hour, session

    public var choice: ApproveChoice { self == .hour ? .autoHour : .autoSession }
}

/// The rows the sheet shows for one proposal.
public struct ApprovePlan: Equatable, Sendable {
    public let tops: [ApproveTop]
    /// The scope rows, in the order the host listed them (exact first, then the prefixes).
    public let scopes: [ApproveScope]
    public let windows: [ApproveWindow]
    public let limit: ApproveChoices.Limit?
    public let floorEntry: String?

    init(_ choices: ApproveChoices) {
        let limited = choices.limit != nil
        var scopes: [ApproveScope] = []
        var windows: [ApproveWindow] = []
        if !limited {
            if choices.exact { scopes.append(.exact) }
            // An empty prefix would match every command and cannot be sent: it is not offered.
            for prefix in choices.prefixes where !prefix.raw.isEmpty { scopes.append(.prefix(raw: prefix.raw)) }
            if choices.auto.contains(.hour) { windows.append(.hour) }
            if choices.auto.contains(.session) { windows.append(.session) }
        }
        var tops: [ApproveTop] = []
        if choices.once { tops.append(.once) }
        if !scopes.isEmpty { tops.append(.similar) }
        if !windows.isEmpty { tops.append(.auto) }
        self.tops = tops
        self.scopes = scopes
        self.windows = windows
        limit = choices.limit
        floorEntry = choices.floorEntry
    }

    var offersAnything: Bool { !tops.isEmpty }

    /// One choice the host lists, to ask whether approving is available at all.
    public var probe: ApproveChoice {
        if tops.contains(.once) { return .once }
        if let scope = scopes.first { return scope.choice }
        return windows.first?.choice ?? .once
    }

    /// The sentence that says why only Approve once is offered; nil when the host limits nothing.
    public var limitText: String? {
        switch limit {
        case nil: return nil
        case .floor?: return ApproveStrings.limitedFloor
        case .restart?: return ApproveStrings.limitedRestart
        case .truncated?: return ApproveStrings.limitedTruncated
        case .notChecked?: return ApproveStrings.limitedNotChecked
        case .textMismatch?: return ApproveStrings.limitedTextMismatch
        case .other?: return ApproveStrings.limitedLead
        }
    }
}

/// The sentence for a deny-only item.
public func approveDenyOnlyText(_ limit: ApproveChoices.Limit?) -> String? {
    switch limit {
    case .truncated?: return ApproveStrings.limitedTruncated
    case .textMismatch?: return ApproveStrings.limitedTextMismatch
    default: return nil
    }
}
