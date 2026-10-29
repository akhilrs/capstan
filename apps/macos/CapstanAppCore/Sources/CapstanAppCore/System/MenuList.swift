import CapstanKit
import Foundation

/// The rows the menu bar shows: the newest few that still wait for the person, and how many more there are.
public struct MenuList: Equatable, Sendable {
    public static let defaultLimit = 5

    public let shown: [InboxRow]
    public let moreCount: Int

    /// Rows already forwarded to a terminal are left out, as the count in the menu bar leaves them out.
    public init(rows: [InboxRow], limit: Int = MenuList.defaultLimit) {
        let waiting = rows.filter { !$0.forwarded }.sorted { left, right in
            let a = MenuList.date(left.since), b = MenuList.date(right.since)
            if let a, let b, a != b { return a > b }
            if left.since != right.since { return left.since > right.since }
            return left.id.key > right.id.key
        }
        shown = Array(waiting.prefix(max(limit, 0)))
        moreCount = max(waiting.count - shown.count, 0)
    }

    /// `since` as a date; nil when it is not ISO 8601, with or without fractional seconds.
    static func date(_ text: String) -> Date? {
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text) ?? plain.date(from: text)
    }

    /// A short age: "now", "5m", "3h", "2d". Empty when the time is not readable.
    public static func age(since: String, now: Date) -> String {
        guard let start = date(since) else { return "" }
        let seconds = Int(now.timeIntervalSince(start))
        if seconds < 60 { return "now" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        if seconds < 86400 { return "\(seconds / 3600)h" }
        return "\(seconds / 86400)d"
    }

    /// The SF Symbol of a kind.
    public static func symbol(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "terminal"
        case .prompt: return "hand.raised"
        case .design: return "paintbrush"
        case .message: return "bubble.left"
        case .integration: return "arrow.triangle.merge"
        case .other: return "questionmark.circle"
        }
    }

    /// The one-line summary: plain text cut to a byte budget; a fixed sentence when the host could not vouch for it.
    public static func summaryLine(_ row: InboxRow, maxBytes: Int = 160) -> String {
        guard row.summary.verified else { return "Text not shown" }
        return TextBudget.displayPrefix(row.summary.displayPlain, maxBytes: maxBytes).text
    }

    public static func projectLine(_ row: InboxRow) -> String {
        let plain = UntrustedText(raw: row.projectName, verified: true, truncated: false).displayPlain
        return TextBudget.displayPrefix(plain, maxBytes: 64).text
    }
}
