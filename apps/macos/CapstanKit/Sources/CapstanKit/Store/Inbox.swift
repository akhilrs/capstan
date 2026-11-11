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
    /// The set an integration row stands for, when the host sent one. The row is the member the person decides next, and
    /// it stands for the whole set: the other members are not rows of their own.
    public let set: IntegrationSet?

    public init(
        id: InboxRowID, kind: WaitingKind, projectName: String, since: String, summary: UntrustedText,
        needsTerminal: Bool, forwarded: Bool, stale: Bool, requestId: String, contentHash: String,
        set: IntegrationSet? = nil
    ) {
        self.set = set
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
                    requestId: item.requestId, contentHash: item.contentHash, set: item.set))
            }
        }
    }
    rows = foldedSetRows(sortedInboxRows(rows))
    return WaitingKind.inboxOrder.compactMap { kind in
        let members = rows.filter { $0.kind == kind }
        return members.isEmpty ? nil : InboxGroup(kind: kind, title: kind.groupTitle, rows: members)
    }
}

/// The number of items that wait for the person and are not yet forwarded.
public func badgeCount(_ hosts: [HostModel]) -> Int {
    inbox(hosts, filter: InboxFilter(includeForwarded: false)).reduce(0) { $0 + $1.count }
}

/// A set shows as one row. Of the member rows of one set (same host, project and set id) the row of the member that is
/// next in merge order stays, else the first in merge order; the others fold into it. The first row keeps the place of
/// the set in the list. A row without a set is left as it is, so a project without `[repos]` is unchanged.
func foldedSetRows(_ rows: [InboxRow]) -> [InboxRow] {
    struct SetKey: Hashable {
        let host: UUID
        let project: String
        let setId: String
    }
    // The member rows of one set carry snapshots that may disagree (each was sent at its own time). The choice must not
    // depend on which row is read first, or the kept row, its id, would jump between updates and drop the selection:
    // take the most advanced snapshot (most members in main, then most built, then the lowest repo name), read the next
    // repo from it, and keep the row of that repo; if no row is of it, the lowest position, then the lowest key.
    var groups: [SetKey: [InboxRow]] = [:]
    for row in rows {
        guard let set = row.set else { continue }
        groups[SetKey(host: row.id.hostId, project: row.id.projectId, setId: set.setId), default: []].append(row)
    }
    var chosen: [SetKey: InboxRow] = [:]
    for (key, members) in groups {
        let sets = members.compactMap(\.set)
        let lead = sets.min {
            ($1.inMainCount, $1.builtCount, $0.repo) < ($0.inMainCount, $0.builtCount, $1.repo)
        }
        let target = lead?.next?.repo
        chosen[key] = members.min { lhs, rhs in
            let l = (lhs.set?.repo == target ? 0 : 1, lhs.set?.position ?? 0, lhs.id.key)
            let r = (rhs.set?.repo == target ? 0 : 1, rhs.set?.position ?? 0, rhs.id.key)
            return l < r
        }
    }
    var placed: Set<SetKey> = []
    var folded: [InboxRow] = []
    for row in rows {
        guard let set = row.set else {
            folded.append(row)
            continue
        }
        let key = SetKey(host: row.id.hostId, project: row.id.projectId, setId: set.setId)
        guard placed.insert(key).inserted, let kept = chosen[key] else { continue }
        folded.append(kept)
    }
    return folded
}
