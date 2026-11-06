import Foundation

/// The rows of the approval choices of one Operator proposal, in the order both apps show them. No UI strings: the
/// apps word the rows themselves.
public enum ApproveOptions {
    public struct Row: Sendable, Equatable {
        public let choice: ApproveChoice
        /// False when the host does not offer this choice for this proposal.
        public let enabled: Bool
        /// Why the proposal is held back (the same for every disabled row of one item); nil when nothing holds it.
        public let limit: ApproveChoices.Limit?
        /// The floor entry named by the host, for `limit` floor.
        public let floorEntry: String?
    }

    /// once, exact, each offered prefix, auto hour, auto session. Empty for an item without `approveChoices`
    /// (another kind, or an old host, which is offered `op approve` only).
    public static func rows(_ item: WaitingItem) -> [Row] {
        guard let choices = item.approveChoices else { return [] }
        func row(_ choice: ApproveChoice) -> Row {
            Row(choice: choice, enabled: offers(choices, choice), limit: choices.limit, floorEntry: choices.floorEntry)
        }
        var rows = [row(.once), row(.exact)]
        rows += choices.prefixes.map { row(.prefix(raw: $0.raw)) }
        rows += [row(.autoHour), row(.autoSession)]
        return rows
    }

    /// Whether the host lists this choice. A prefix must equal an offered one byte for byte.
    static func offers(_ choices: ApproveChoices, _ choice: ApproveChoice) -> Bool {
        // A command that no longer matches its hash allows nothing but a deny.
        if choices.limit == .textMismatch { return false }
        switch choice {
        case .once: return choices.once
        case .exact: return choices.exact
        case .prefix(let raw):
            let wanted = Array(raw.utf8)
            return choices.prefixes.contains { Array($0.raw.utf8) == wanted }
        case .autoHour: return choices.auto.contains(.hour)
        case .autoSession: return choices.auto.contains(.session)
        }
    }
}
