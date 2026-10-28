import CapstanKit
import Foundation

public struct InboxScope: Sendable, Equatable {
    public var kind: WaitingKind?
    public var hostId: UUID?
    public var projectId: String?

    public init(kind: WaitingKind? = nil, hostId: UUID? = nil, projectId: String? = nil) {
        self.kind = kind
        self.hostId = hostId
        self.projectId = projectId
    }

    /// The groups with only the rows in scope; groups left without rows are dropped.
    public func apply(_ groups: [InboxGroup]) -> [InboxGroup] {
        groups.compactMap { group in
            let rows = group.rows.filter { row in
                if let kind, row.kind != kind { return false }
                if let hostId, row.id.hostId != hostId { return false }
                if let projectId, row.id.projectId != projectId { return false }
                return true
            }
            return rows.isEmpty ? nil : InboxGroup(kind: group.kind, title: group.title, rows: rows)
        }
    }
}

/// A cap in UTF-8 bytes, the unit of the protocol limits.
public struct TextBudget: Sendable, Equatable {
    /// The cap for free text sent to a host: it is refused over the cap, never cut.
    public static let reason = TextBudget(limit: ProtocolLimits.reasonCap)

    public let limit: Int

    public init(limit: Int) {
        self.limit = limit
    }

    public func used(_ s: String) -> Int {
        s.utf8.count
    }

    public func allows(_ s: String) -> Bool {
        used(s) <= limit
    }

    /// For display only: the longest start of the text that fits, cut between characters.
    public static func displayPrefix(_ s: String, maxBytes: Int) -> (text: String, cut: Bool) {
        var bytes = 0
        var end = s.startIndex
        for index in s.indices {
            let next = s.index(after: index)
            let size = s[index..<next].utf8.count
            if bytes + size > maxBytes {
                return (String(s[s.startIndex..<end]), true)
            }
            bytes += size
            end = next
        }
        return (s, false)
    }
}

public enum PendingResolution: Sendable, Equatable {
    case select, wait, expire
}

/// A row the person asked to open (from a notification, say) that may not have arrived yet.
public struct PendingRow: Sendable, Equatable {
    public static let waitSeconds: TimeInterval = 30

    public let row: InboxRowID
    public let since: Date

    public init(row: InboxRowID, since: Date) {
        self.row = row
        self.since = since
    }

    public func resolve(rows: Set<InboxRowID>, now: Date) -> PendingResolution {
        if rows.contains(row) { return .select }
        return now.timeIntervalSince(since) < PendingRow.waitSeconds ? .wait : .expire
    }
}

/// The colour forms of the design's token tables.
public enum TokenValue {
    public static func parse(
        _ s: String, ink: (r: Double, g: Double, b: Double)
    ) -> (r: Double, g: Double, b: Double, a: Double)? {
        let text = s.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("#") { return hex(String(text.dropFirst())) }
        if text.hasPrefix("rgba("), text.hasSuffix(")") {
            return rgba(String(text.dropFirst(5).dropLast()))
        }
        if text.hasSuffix("% ink"), let alpha = percent(String(text.dropLast(5))) {
            return (ink.r, ink.g, ink.b, alpha)
        }
        if text.hasSuffix("% white"), let alpha = percent(String(text.dropLast(7))) {
            return (1, 1, 1, alpha)
        }
        return nil
    }

    private static func hex(_ digits: String) -> (r: Double, g: Double, b: Double, a: Double)? {
        guard digits.count == 6, digits.allSatisfy({ $0.isHexDigit }), let value = Int(digits, radix: 16) else {
            return nil
        }
        return (
            Double((value >> 16) & 0xFF) / 255, Double((value >> 8) & 0xFF) / 255, Double(value & 0xFF) / 255, 1
        )
    }

    private static func rgba(_ body: String) -> (r: Double, g: Double, b: Double, a: Double)? {
        let parts = body.split(separator: ",", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 4,
            let red = Double(parts[0]), let green = Double(parts[1]), let blue = Double(parts[2]),
            let alpha = Double(parts[3]),
            (0...255).contains(red), (0...255).contains(green), (0...255).contains(blue), (0...1).contains(alpha)
        else { return nil }
        return (red / 255, green / 255, blue / 255, alpha)
    }

    private static func percent(_ number: String) -> Double? {
        guard let value = Double(number.trimmingCharacters(in: .whitespaces)), (0...100).contains(value) else {
            return nil
        }
        return value / 100
    }
}
