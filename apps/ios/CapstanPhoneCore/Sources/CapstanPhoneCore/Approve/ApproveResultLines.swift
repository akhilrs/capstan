import CapstanKit
import Foundation

/// The lines of the detail after an approval was applied, from `DecideResult.approval`. A grant or a window the host
/// did not make is not claimed: each part is shown only when the result carries it.
public struct ApproveResultLines: Equatable, Sendable {
    public let headline: String
    /// What was allowed for the session: the exact command or the prefix, as chosen; nil when no grant was made.
    public let grantText: String?
    public let windowId: String?
    public let ends: String?
    public let revoke: String?
    /// "The PM was told…", always.
    public let pm: String

    /// - Parameters:
    ///   - approval: the result's `approval`; nil from a host that sent none.
    ///   - chosen: what the person chose, for the grant text (the host's result does not repeat it).
    public static func make(approval: ApprovalResult?, chosen: ApproveChoice?, command: String) -> ApproveResultLines {
        let pm = ApproveStrings.resultPm
        guard let approval else {
            return ApproveResultLines(headline: ApproveStrings.resultOnce, grantText: nil, windowId: nil, ends: nil, revoke: nil, pm: pm)
        }
        if approval.windowId != nil || approval.choice.hasPrefix("auto") {
            return ApproveResultLines(
                headline: ApproveStrings.resultWindow, grantText: nil,
                windowId: approval.windowId.map { ApproveStrings.resultWindowId.replacingOccurrences(of: "{windowId}", with: $0) },
                ends: approval.expiresAt.map { ApproveStrings.resultEnds.replacingOccurrences(of: "{time}", with: hostTime($0)) },
                revoke: ApproveStrings.resultWindowRevoke, pm: pm)
        }
        if approval.grantId != nil {
            let text: String
            switch chosen {
            case .prefix(let raw)?: text = raw
            default: text = command
            }
            return ApproveResultLines(headline: ApproveStrings.resultGrant, grantText: text, windowId: nil, ends: nil, revoke: nil, pm: pm)
        }
        return ApproveResultLines(headline: ApproveStrings.resultOnce, grantText: nil, windowId: nil, ends: nil, revoke: nil, pm: pm)
    }

    /// The time of day as the host wrote it (`2026-11-06T14:05:00+05:30` gives `14:05`): host time, not converted.
    static func hostTime(_ stamp: String) -> String {
        guard let t = stamp.firstIndex(of: "T") else { return stamp }
        let rest = stamp[stamp.index(after: t)...]
        let clock = rest.prefix(5)
        guard clock.count == 5, clock[clock.index(clock.startIndex, offsetBy: 2)] == ":" else { return stamp }
        return String(clock)
    }
}

/// What the sheet says about a refusal of `op approve-as`. Nothing was approved in any of these.
public struct ApproveRefusalLines: Equatable, Sendable {
    public enum Action: Equatable, Sendable {
        case none
        /// Read the command again (`hash_mismatch`).
        case showNew
    }

    public let text: String
    public let action: Action
    /// The proposal is no longer waiting: the sheet's actions give way to this text.
    public let replacesActions: Bool
    /// The host's own words, shown apart as untrusted text.
    public let hostText: UntrustedText?

    /// nil for an outcome the sheet does not word itself (sent, unreachable, unreadable): the detail's banner does.
    public static func make(_ outcome: DecideOutcome) -> ApproveRefusalLines? {
        guard case .refused(let refusal, _) = outcome else { return nil }
        func lines(_ text: String, _ action: Action = .none, replaces: Bool = false, host: Bool = false) -> ApproveRefusalLines {
            ApproveRefusalLines(
                text: text, action: action, replacesActions: replaces,
                hostText: host && !refusal.message.raw.isEmpty ? refusal.message : nil)
        }
        switch refusal.code {
        case .hashMismatch: return lines(ApproveStrings.refusalHashMismatch, .showNew)
        case .notWaiting: return lines(ApproveStrings.refusalNotWaiting, replaces: true)
        case .needsTerminal: return lines(ApproveStrings.refusalNeedsTerminal)
        case .invalidRequest: return lines(ApproveStrings.refusalInvalidRequest, host: true)
        case .other(let code) where code == "local-busy" || code == "busy" || code == "operator_busy":
            return lines(ApproveStrings.refusalBusy)
        default: return nil
        }
    }

    /// "Nothing was approved." and then `text`.
    public var lead: String { ApproveStrings.refusalNothing }
}
