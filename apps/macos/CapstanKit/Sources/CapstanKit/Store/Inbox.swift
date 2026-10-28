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

/// `since` as a date; nil when it is not ISO 8601 UTC, with or without fractional seconds.
private func sinceDate(_ text: String) -> Date? {
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return fractional.date(from: text) ?? plain.date(from: text)
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
    rows.sort { left, right in
        if left.since != right.since {
            if let a = sinceDate(left.since), let b = sinceDate(right.since), a != b { return a < b }
            return left.since < right.since
        }
        if left.id.hostId != right.id.hostId { return left.id.hostId.uuidString < right.id.hostId.uuidString }
        if left.id.projectId != right.id.projectId { return left.id.projectId < right.id.projectId }
        return left.id.key < right.id.key
    }
    return WaitingKind.inboxOrder.compactMap { kind in
        let members = rows.filter { $0.kind == kind }
        return members.isEmpty ? nil : InboxGroup(kind: kind, title: kind.groupTitle, rows: members)
    }
}

/// The number of items that wait for the person and are not yet forwarded.
public func badgeCount(_ hosts: [HostModel]) -> Int {
    inbox(hosts, filter: InboxFilter(includeForwarded: false)).reduce(0) { $0 + $1.count }
}
