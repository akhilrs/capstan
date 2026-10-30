import Foundation

/// What can ask for the history of a project. Polls and live events are not among them: they never start a read.
public enum RecentTriggerEvent: Sendable, Equatable {
    case connected(hostId: UUID)
    case disconnected(hostId: UUID)
    case selected(projectId: String)
    /// The person asked to refresh.
    case refreshed(projectId: String)
    /// The project was resynced; `head` is its new head.
    case resynced(projectId: String, head: Int64)
    case answered(projectId: String, RecentResult)
}

/// The epoch and head the app holds for a project.
public struct RecentPosition: Sendable, Equatable {
    public let epoch: String
    public let head: Int64

    public init(epoch: String, head: Int64) {
        self.epoch = epoch
        self.head = head
    }
}

/// Decides when to call `cstan recent`: once per selected project per connection, on Refresh, and again at the new
/// head after a resync. It holds no clock and starts nothing; the caller runs the calls it returns.
public struct RecentTrigger: Sendable, Equatable {
    private var connection: UUID?
    private var selected: String?
    /// The projects already read on this connection.
    private var called: Set<String> = []
    /// The projects whose host has no `recent` command, on this connection.
    private var sessionOnly: Set<String> = []
    /// After a rate-limited answer: the earliest time a project may be read again.
    private var notBefore: [String: Date] = [:]

    public init() {}

    /// Records when the host's rate limit window ends for a project; Refresh does nothing before it.
    public mutating func limit(projectId: String, until date: Date) {
        notBefore[projectId] = date
    }

    /// The calls to run for `event`. `position` gives the epoch and head the app holds for a project; a selected
    /// project without one is read as soon as the event is fed again (the caller feeds the selection again when the
    /// project's snapshot arrives).
    public mutating func handle(
        _ event: RecentTriggerEvent, now: Date = Date(), position: (String) -> RecentPosition?
    ) -> [RecentCall] {
        switch event {
        case .connected(let hostId):
            connection = hostId
            called = []
            sessionOnly = []
            guard let project = selected else { return [] }
            return call(project, head: nil, position: position)
        case .disconnected(let hostId):
            if connection == hostId {
                connection = nil
                called = []
                sessionOnly = []
            }
            return []
        case .selected(let projectId):
            selected = projectId
            if called.contains(projectId) { return [] }
            return call(projectId, head: nil, position: position)
        case .refreshed(let projectId):
            // A host may have been updated since it answered that it has no such command.
            sessionOnly.remove(projectId)
            if let until = notBefore[projectId], now < until { return [] }
            notBefore[projectId] = nil
            return call(projectId, head: nil, position: position)
        case .resynced(let projectId, let head):
            called.remove(projectId)
            return call(projectId, head: head, position: position)
        case .answered(let projectId, let result):
            if result == .sessionOnly { sessionOnly.insert(projectId) }
            return []
        }
    }

    private mutating func call(
        _ projectId: String, head: Int64?, position: (String) -> RecentPosition?
    ) -> [RecentCall] {
        guard connection != nil, selected == projectId, !sessionOnly.contains(projectId),
              let held = position(projectId) else { return [] }
        called.insert(projectId)
        return [RecentCall(projectId: projectId, epoch: held.epoch, head: head ?? held.head)]
    }
}
