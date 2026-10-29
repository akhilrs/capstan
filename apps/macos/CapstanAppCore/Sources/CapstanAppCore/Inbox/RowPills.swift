import Foundation

public enum Pill: Sendable, Equatable {
    case forwarded, terminalOnly, stale

    public var word: String {
        switch self {
        case .forwarded: return "Forwarded"
        case .terminalOnly: return "Terminal only"
        case .stale: return "Stale"
        }
    }
}

public enum RowPills {
    /// The pills of a row, in a fixed order: Terminal only, Forwarded, Stale.
    public static func make(needsTerminal: Bool, forwarded: Bool, stale: Bool) -> [Pill] {
        var pills: [Pill] = []
        if needsTerminal { pills.append(.terminalOnly) }
        if forwarded { pills.append(.forwarded) }
        if stale { pills.append(.stale) }
        return pills
    }
}
