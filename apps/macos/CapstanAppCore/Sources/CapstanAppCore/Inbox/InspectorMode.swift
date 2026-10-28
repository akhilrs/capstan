import CapstanKit
import Foundation

public enum InspectorMode: Sendable, Equatable {
    case interactive
    /// The host's cstan cannot take decisions.
    case readOnlyNoDecide
    /// A branch ready to merge: merging is done from the terminal.
    case readOnlyIntegration

    public static func make(decide: Bool, kind: WaitingKind) -> InspectorMode {
        if case .integration = kind { return .readOnlyIntegration }
        return decide ? .interactive : .readOnlyNoDecide
    }

    /// The line that tells the person where to act; nil when the inspector is interactive.
    public var terminalHint: String? {
        switch self {
        case .interactive: return nil
        case .readOnlyNoDecide: return "Answer in the terminal"
        case .readOnlyIntegration: return "Merge from the terminal"
        }
    }
}

public enum ComposerState: Sendable, Equatable {
    case empty, ready, overLimit

    public static func check(_ text: String, budget: TextBudget) -> ComposerState {
        if NoteText.normalized(text) == nil { return .empty }
        return budget.allows(text) ? .ready : .overLimit
    }
}

public enum AgeText {
    /// A short age such as "5m"; "" when `since` is not ISO 8601 UTC.
    public static func short(since: String, now: Date) -> String {
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = fractional.date(from: since) ?? plain.date(from: since) else { return "" }
        let seconds = max(0, Int(now.timeIntervalSince(date)))
        switch seconds {
        case ..<60: return "now"
        case ..<3600: return String(seconds / 60) + "m"
        case ..<86400: return String(seconds / 3600) + "h"
        default: return String(seconds / 86400) + "d"
        }
    }
}
