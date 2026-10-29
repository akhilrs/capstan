import Foundation

extension WaitingKind {
    /// The kinds the inbox shows, in the order of its groups.
    static let inboxOrder: [WaitingKind] = [.op, .prompt, .design, .message, .integration]

    public var groupTitle: String {
        switch self {
        case .op: return "Command proposals"
        case .prompt: return "Prompts"
        case .design: return "Designs"
        case .message: return "Messages"
        case .integration: return "Ready to merge"
        case .other(let name): return name
        }
    }
}

public struct InboxRowID: Hashable, Codable, Sendable {
    public let hostId: UUID
    public let projectId: String
    public let key: String

    public init(hostId: UUID, projectId: String, key: String) {
        self.hostId = hostId
        self.projectId = projectId
        self.key = key
    }
}

public struct InboxRow: Identifiable, Sendable, Equatable {
    public let id: InboxRowID
    public let kind: WaitingKind
    public let projectName: String
    /// The time the item began to wait, as the host wrote it.
    public let since: String
    public let summary: UntrustedText
    public let needsTerminal: Bool
    /// The decision was forwarded to the terminal that owns the item.
    public let forwarded: Bool
    /// The project is not ready; the row is the last one read.
    public let stale: Bool
    public let requestId: String
    public let contentHash: String

    public init(
        id: InboxRowID, kind: WaitingKind, projectName: String, since: String, summary: UntrustedText,
        needsTerminal: Bool, forwarded: Bool, stale: Bool, requestId: String, contentHash: String
    ) {
        self.id = id
        self.kind = kind
        self.projectName = projectName
        self.since = since
        self.summary = summary
        self.needsTerminal = needsTerminal
        self.forwarded = forwarded
        self.stale = stale
        self.requestId = requestId
        self.contentHash = contentHash
    }
}

public struct InboxGroup: Identifiable, Sendable, Equatable {
    public let kind: WaitingKind
    public let title: String
    public let rows: [InboxRow]

    public var id: String { kind.wireName }
    public var count: Int { rows.count }

    public init(kind: WaitingKind, title: String, rows: [InboxRow]) {
        self.kind = kind
        self.title = title
        self.rows = rows
    }
}

public struct InboxFilter: Sendable, Equatable {
    public var hostId: UUID?
    public var projectId: String?
    public var kind: WaitingKind?
    public var includeForwarded: Bool

    public init(hostId: UUID? = nil, projectId: String? = nil, kind: WaitingKind? = nil, includeForwarded: Bool = true) {
        self.hostId = hostId
        self.projectId = projectId
        self.kind = kind
        self.includeForwarded = includeForwarded
    }

    public static let all = InboxFilter()
}

/// Reads `since` as a date. The formatters are made once; the lock keeps one call at a time, as actors share them.
private final class SinceParser: @unchecked Sendable {
    static let shared = SinceParser()

    private let lock = NSLock()
    private let plain: ISO8601DateFormatter
    private let fractional: ISO8601DateFormatter

    private init() {
        plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    }

    /// nil when the text is not ISO 8601 UTC, with or without fractional seconds.
    func date(_ text: String) -> Date? {
        lock.withLock { fractional.date(from: text) ?? plain.date(from: text) }
    }
}

/// The rows oldest first: by date when both dates read, else by the `since` text; then host, project and key.
/// Each date is read once, before the sort.
func sortedInboxRows(_ rows: [InboxRow]) -> [InboxRow] {
    let parser = SinceParser.shared
    let dated = rows.map { (row: $0, date: parser.date($0.since)) }
    let sorted = dated.sorted { left, right in
        if left.row.since != right.row.since {
            if let a = left.date, let b = right.date, a != b { return a < b }
            return left.row.since < right.row.since
        }
        if left.row.id.hostId != right.row.id.hostId { return left.row.id.hostId.uuidString < right.row.id.hostId.uuidString }
        if left.row.id.projectId != right.row.id.projectId { return left.row.id.projectId < right.row.id.projectId }
        return left.row.id.key < right.row.id.key
    }
    return sorted.map(\.row)
}

/// The groups of waiting items, each oldest first. Empty groups are left out.
public func inbox(_ hosts: [HostModel], filter: InboxFilter = .all) -> [InboxGroup] {
    var rows: [InboxRow] = []
    for host in hosts where filter.hostId == nil || filter.hostId == host.hostId {
        for project in host.projects.values where filter.projectId == nil || filter.projectId == project.projectId {
            let records = host.forwards[project.projectId] ?? []
            for item in project.waiting.values {
                if let kind = filter.kind, kind != item.kind { continue }
                let forwarded = records.contains { $0.covers(item) }
                if forwarded && !filter.includeForwarded { continue }
                rows.append(InboxRow(
                    id: InboxRowID(hostId: host.hostId, projectId: project.projectId, key: item.key),
                    kind: item.kind, projectName: project.displayName, since: item.since, summary: item.summary,
                    needsTerminal: item.needsTerminal, forwarded: forwarded, stale: project.staleSince != nil,
                    requestId: item.requestId, contentHash: item.contentHash))
            }
        }
    }
    rows = sortedInboxRows(rows)
    return WaitingKind.inboxOrder.compactMap { kind in
        let members = rows.filter { $0.kind == kind }
        return members.isEmpty ? nil : InboxGroup(kind: kind, title: kind.groupTitle, rows: members)
    }
}

/// The number of items that wait for the person and are not yet forwarded.
public func badgeCount(_ hosts: [HostModel]) -> Int {
    inbox(hosts, filter: InboxFilter(includeForwarded: false)).reduce(0) { $0 + $1.count }
}
