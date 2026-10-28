import CapstanKit
import Foundation

public enum Tone: Sendable, Equatable {
    case neutral, green, blue, yellow, red, grey
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
