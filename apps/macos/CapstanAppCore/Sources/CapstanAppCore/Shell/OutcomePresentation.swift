import CapstanKit
import Foundation

public enum Tone: Sendable, Equatable {
    case neutral, green, blue, yellow, red, grey, accent
}

public enum PresentAction: Sendable, Equatable {
    case refresh, retry, tryAgain
}

public struct Presentation: Sendable, Equatable {
    public let title: String
    public let tone: Tone
    public let actions: [PresentAction]

    public init(title: String, tone: Tone, actions: [PresentAction] = []) {
        self.title = title
        self.tone = tone
        self.actions = actions
    }

    /// The words and colour for the state of one decision; nil means it is being sent.
    public static func present(_ o: DecideOutcome?, hostName: String) -> Presentation {
        guard let o else { return Presentation(title: "Sending", tone: .neutral) }
        switch o {
        case .refused(let refusal, let hint):
            if case .other("local-busy") = refusal.code {
                return Presentation(title: "Already sending", tone: .grey)
            }
            if case .other(let code) = refusal.code, unsupportedCodes.contains(code) {
                return Presentation(
                    title: "This project's Capstan is older than the app. Restart it on the host: cstan restart",
                    tone: .red)
            }
            return refused(refusal, hint)
        case .applied(let result):
            if result.isRepeat { return Presentation(title: "Already sent", tone: .grey) }
            return Presentation(title: "Done", tone: .green)
        case .forwarded(let result):
            if result.isRepeat { return Presentation(title: "Already sent", tone: .grey) }
            return Presentation(title: "Forwarded to the PM, waiting", tone: .blue)
        case .notSent:
            return Presentation(title: "Could not reach " + clean(hostName), tone: .red, actions: [.retry])
        case .malformed:
            return Presentation(title: "Unexpected answer from the host", tone: .red, actions: [.retry])
        }
    }

    /// What an older controller answers to a verb it does not know.
    private static let unsupportedCodes: Set<String> = ["unknown_command", "unsupported"]

    private static func refused(_ refusal: DecideRefusal, _ hint: RefusalHint) -> Presentation {
        switch hint {
        case .terminal: return Presentation(title: "Answer in the terminal", tone: .yellow)
        case .refresh: return Presentation(title: "This item changed", tone: .yellow, actions: [.refresh])
        case .gone: return Presentation(title: "No longer waiting", tone: .grey)
        case .noPm: return Presentation(title: "No active PM in this project", tone: .red)
        case .limit: return Presentation(title: "Too many forwards for this item", tone: .yellow)
        case .internal: return Presentation(title: "Could not send", tone: .red, actions: [.tryAgain])
        case .invalid: return Presentation(title: "The host rejected the request", tone: .red)
        case .host:
            return Presentation(title: "The host refused: " + clean(wireString(refusal.code)), tone: .red)
        case .generic:
            return Presentation(title: "Refused: " + clean(wireString(refusal.code)), tone: .red)
        }
    }

    /// Host text as plain, cut to a display size.
    private static func clean(_ s: String) -> String {
        let plain = UntrustedText(raw: s, verified: true, truncated: false).displayPlain
        return TextBudget.displayPrefix(plain, maxBytes: 64).text
    }

    private static func wireString(_ code: DecideErrorCode) -> String {
        switch code {
        case .notWaiting: return "not_waiting"
        case .hashMismatch: return "hash_mismatch"
        case .nonceReused: return "nonce_reused"
        case .needsTerminal: return "needs_terminal"
        case .noActivePm: return "no_active_pm"
        case .limitReached: return "limit_reached"
        case .invalidRequest: return "invalid_request"
        case .forbidden: return "forbidden"
        case .unauthorized: return "unauthorized"
        case .projectNotFound: return "project_not_found"
        case .noOperatorKey: return "no_operator_key"
        case .notAnswering: return "not_answering"
        case .other(let text): return text
        }
    }
}

/// The result of an `op approve-as` in the words of the design: what was applied, or why nothing was.
public enum ApprovePresentation: Sendable, Equatable {
    public struct Applied: Sendable, Equatable {
        /// "Approved once. The operator runs it now." and the like.
        public let lead: String
        /// The prefix as received, shown whole in its box (grant by prefix).
        public let prefix: String?
        /// "This exact command" for a grant by the exact command.
        public let scopeText: String?
        public let windowLine: String?
        /// "Ends 16:58", from the time the host sent; nil when it sent none.
        public let endsLine: String?
        public let revokeLine: String?
        public let pmLine: String
    }

    public struct Refused: Sendable, Equatable {
        public enum Tone: Sendable, Equatable { case yellow, grey, red }
        public let tone: Tone
        public let text: String
        /// "Nothing was approved."
        public let nothing: String
        public let code: String
        /// The sheet closes on its own.
        public let closesSheet: Bool
        /// The primary stays live so the person may try again.
        public let primaryLive: Bool
        /// A button label, e.g. "Show the new command".
        public let action: String?
        /// The host's message is shown in an untrusted block.
        public let showsHostMessage: Bool
        /// The host's message (host text), set when `showsHostMessage` is true.
        public let hostMessage: UntrustedText?
    }

    case sending
    case applied(Applied)
    case refused(Refused)
    /// Any other state: use `Presentation.present`.
    case other(Presentation)

    /// `sent` is the choice that was sent; its prefix is the one the person chose.
    public static func present(_ outcome: DecideOutcome?, sent: ApproveChoice, hostName: String) -> ApprovePresentation {
        guard let outcome else { return .sending }
        switch outcome {
        case .applied(let result) where !result.isRepeat:
            guard let approval = result.approval else { return .other(Presentation.present(outcome, hostName: hostName)) }
            return .applied(applied(approval, sent: sent))
        case .refused(let refusal, _):
            if let refused = refused(refusal) { return .refused(refused) }
            return .other(Presentation.present(outcome, hostName: hostName))
        default:
            return .other(Presentation.present(outcome, hostName: hostName))
        }
    }

    private static func applied(_ approval: ApprovalResult, sent: ApproveChoice) -> Applied {
        let ends = approval.expiresAt.map { ApproveStrings.resultEnds.replacingOccurrences(of: "{time}", with: hostClock($0)) }
        func make(
            _ lead: String, prefix: String? = nil, scope: String? = nil, window: String? = nil, ends: String? = nil,
            revoke: String? = nil
        ) -> Applied {
            Applied(
                lead: lead, prefix: prefix, scopeText: scope, windowLine: window, endsLine: ends, revokeLine: revoke,
                pmLine: ApproveStrings.resultPm)
        }
        switch approval.choice {
        case "exact": return make(ApproveStrings.resultGrant, scope: ApproveStrings.scopeExactTitle, ends: ends)
        case "prefix":
            if case .prefix(let raw) = sent { return make(ApproveStrings.resultGrant, prefix: raw, ends: ends) }
            return make(ApproveStrings.resultGrant, ends: ends)
        case "auto-hour", "auto-session":
            let id = approval.windowId.map { ApproveStrings.resultWindowId.replacingOccurrences(of: "{windowId}", with: $0) }
            return make(ApproveStrings.resultWindow, window: id, ends: ends, revoke: ApproveStrings.resultWindowRevoke)
        default: return make(ApproveStrings.resultOnce)
        }
    }

    /// The clock part of the host's time with the zone it was sent in (`2026-11-05T16:58:00+05:30` is
    /// `16:58 +05:30`, `...Z` is `14:41 UTC`); the whole text, cut to a display size, when it has no clock.
    /// Nothing is converted on this device, so the zone is named.
    static func hostClock(_ iso: String) -> String {
        if let t = iso.firstIndex(of: "T") {
            let rest = iso[iso.index(after: t)...]
            if rest.count >= 5, rest[rest.index(rest.startIndex, offsetBy: 2)] == ":" {
                let clock = String(rest.prefix(5))
                let tail = rest.dropFirst(5)
                if tail.hasSuffix("Z") { return clock + " UTC" }
                if let z = tail.lastIndex(where: { $0 == "+" || $0 == "-" }) {
                    let zone = String(tail[z...])
                    if zone.count >= 3, zone.dropFirst().allSatisfy({ $0.isNumber || $0 == ":" }) { return clock + " " + zone }
                }
                return clock
            }
        }
        return TextBudget.displayPrefix(UntrustedText(raw: iso, verified: true, truncated: false).displayPlain, maxBytes: 64).text
    }

    private static func refused(_ refusal: DecideRefusal) -> Refused? {
        func make(
            _ tone: Refused.Tone, _ text: String, _ code: String, closes: Bool = false, live: Bool = false,
            action: String? = nil, host: Bool = false
        ) -> Refused {
            Refused(
                tone: tone, text: text, nothing: ApproveStrings.refusalNothing, code: code, closesSheet: closes,
                primaryLive: live, action: action, showsHostMessage: host, hostMessage: host ? refusal.message : nil)
        }
        switch refusal.code {
        case .other(let code) where code == "local-busy" || code == "busy":
            return make(.yellow, ApproveStrings.refusalBusy, code, live: true)
        case .notWaiting: return make(.grey, ApproveStrings.refusalNotWaiting, "not_waiting", closes: true)
        case .hashMismatch:
            return make(
                .red, ApproveStrings.refusalHashMismatch, "hash_mismatch", action: ApproveStrings.refusalHashMismatchAction)
        case .needsTerminal: return make(.red, ApproveStrings.refusalNeedsTerminal, "needs_terminal", host: true)
        case .invalidRequest: return make(.red, ApproveStrings.refusalInvalidRequest, "invalid_request")
        default: return nil
        }
    }
}
