import CapstanAppCore
import CapstanKit
import Foundation

/// The phone's words and haptics for the result of a decision (board 05). The title, tone and actions come from
/// `Presentation` in CapstanAppCore, shared with the Mac; this adds the line under the title, with the protocol's code
/// where the board names it, the glyph, and the haptic. No host or agent text: that is drawn separately as untrusted.
enum OutcomeLines {
    /// The sentence under the title; `at` is when this phone received the result.
    static func detail(_ outcome: DecideOutcome, at: Date) -> String? {
        let time = at.formatted(date: .omitted, time: .shortened)
        switch outcome {
        case .applied(let result):
            if result.isRepeat { return "repeat: the host had this answer; nothing new was sent." }
            return "Applied on the host at " + time + "."
        case .forwarded(let result):
            if result.isRepeat { return "repeat: the host had this answer; nothing new was sent." }
            return "The PM asks you to confirm before anything runs. " + time
        case .refused(let refusal, let hint):
            if case .other("local-busy") = refusal.code { return "Another answer to this item is being sent." }
            switch hint {
            case .refresh:
                return "The host refused the answer (hash_mismatch). Nothing was sent. Read the new text, then answer again."
            case .gone: return "not_waiting: it was answered or withdrawn before your answer arrived."
            case .terminal: return "needs_terminal: this answer cannot be sent from the phone."
            case .noPm: return "no_active_pm: start a PM, or answer in the terminal."
            case .limit: return "limit_reached: this item already has as many messages to the PM as it can take."
            case .internal: return "Nothing was applied. Try again sends it with a new request."
            case .invalid, .host, .generic: return "Nothing was applied."
            }
        case .notSent(let reason, _):
            switch reason {
            case .couldNotStart: return "The connection could not start. Nothing was sent."
            case .timeout: return "No answer came in time. Retry sends the same request; it cannot be applied twice."
            case .ssh: return "SSH failed (exit 255). Nothing was sent."
            }
        case .malformed:
            return "The answer could not be read. Retry sends the same request; it cannot be applied twice."
        }
    }

    static func symbol(_ outcome: DecideOutcome) -> String {
        switch outcome {
        case .applied(let result): return result.isRepeat ? "arrow.clockwise" : "checkmark"
        case .forwarded(let result): return result.isRepeat ? "arrow.clockwise" : "arrow.up"
        case .refused(_, let hint):
            switch hint {
            case .refresh: return "arrow.clockwise"
            case .gone: return "checkmark.circle"
            case .terminal: return "apple.terminal"
            case .noPm, .limit: return "exclamationmark.triangle"
            case .internal, .invalid, .host, .generic: return "exclamationmark.circle"
            }
        case .notSent: return "wifi.exclamationmark"
        case .malformed: return "exclamationmark.circle"
        }
    }

    enum Feel: Equatable { case success, warning, error }

    /// The design's haptics: success when applied or forwarded; warning on hash_mismatch, needs_terminal and
    /// limit_reached; error when the host could not be reached; none for the rest.
    static func feel(_ outcome: DecideOutcome) -> Feel? {
        switch outcome {
        case .applied(let result), .forwarded(let result): return result.isRepeat ? nil : .success
        case .refused(_, let hint):
            switch hint {
            case .refresh, .terminal, .limit: return .warning
            default: return nil
            }
        case .notSent, .malformed: return .error
        }
    }

    /// Host words worth showing under the line: the ssh error, or the reason of a refusal the app has no words for.
    static func hostText(_ outcome: DecideOutcome) -> UntrustedText? {
        switch outcome {
        case .notSent(.ssh(let tail), _): return tail
        case .refused(let refusal, let hint):
            switch hint {
            case .invalid, .host, .generic: return refusal.message.raw.isEmpty ? nil : refusal.message
            default: return nil
            }
        default: return nil
        }
    }

    /// A new forward, which the toast announces.
    static func isNewForward(_ outcome: DecideOutcome) -> Bool {
        if case .forwarded(let result) = outcome { return !result.isRepeat }
        return false
    }

    /// The decision went through: the bar offers nothing more to send.
    static func isDone(_ outcome: DecideOutcome) -> Bool {
        switch outcome {
        case .applied, .forwarded: return true
        default: return false
        }
    }

    static func pendingId(_ outcome: DecideOutcome) -> UUID? {
        switch outcome {
        case .notSent(_, let id): return id
        case .malformed(_, let id): return id
        default: return nil
        }
    }
}
