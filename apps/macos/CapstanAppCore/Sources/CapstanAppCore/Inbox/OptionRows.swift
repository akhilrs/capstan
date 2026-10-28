import CapstanKit
import Foundation

public enum OptionRowState: Sendable, Equatable {
    /// The person can choose it in the app.
    case selectable
    /// It changes permissions beyond the action; terminal only.
    case widening
    /// It takes typed text; terminal only.
    case text
}

public struct OptionRow: Sendable, Equatable, Identifiable {
    public let number: Int64
    public let text: UntrustedText
    public let state: OptionRowState

    public var id: Int64 { number }

    /// The words shown beside a row that cannot be chosen here; nil for a selectable row.
    public var note: String? {
        switch state {
        case .selectable: return nil
        case .widening: return "Changes permissions beyond this action \u{00B7} terminal only"
        case .text: return "Takes typed text \u{00B7} terminal only"
        }
    }

    public init(number: Int64, text: UntrustedText, state: OptionRowState) {
        self.number = number
        self.text = text
        self.state = state
    }
}

public enum OptionRows {
    /// One row per option. Widening wins over text. An item that needs a terminal has no selectable row: only Esc.
    public static func make(_ options: [PromptOption], needsTerminal: Bool) -> [OptionRow] {
        options.map { option in
            let state: OptionRowState
            if option.widensPermissions {
                state = .widening
            } else if option.acceptsText || needsTerminal {
                state = .text
            } else {
                state = .selectable
            }
            return OptionRow(number: option.number, text: option.text, state: state)
        }
    }
}
