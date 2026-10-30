import Foundation

// The three signals of how far the overview can be trusted, kept apart: the liveness of the connection (host-wide), the
// availability of the project, and the completeness of the project's data. Only the first one reads a clock for elapsed
// time, and that clock is monotonic; the other two are stamped with the local receipt time, which is for showing.

/// A clock that never moves backwards and does not follow changes of the wall clock. Its origin is arbitrary.
public protocol MonotonicClock: Sendable {
    func seconds() -> TimeInterval
}

/// The production clock: `ContinuousClock`.
public struct ContinuousMonotonicClock: MonotonicClock {
    private let origin: ContinuousClock.Instant

    public init() {
        origin = ContinuousClock.now
    }

    public func seconds() -> TimeInterval {
        let elapsed: Duration = origin.duration(to: ContinuousClock.now)
        return TimeInterval(elapsed.components.seconds) + TimeInterval(elapsed.components.attoseconds) / 1e18
    }
}

public enum ConnectionLiveness: Sendable, Equatable {
    /// No line was received yet on this connection.
    case unknown
    case live
    /// Nothing, not even a heartbeat, was received for the missed intervals.
    case silent
}

/// When the host last sent any line. A heartbeat counts here, and nowhere else.
public struct LivenessTracker: Sendable, Equatable {
    /// The intervals of silence after which the host counts as silent.
    public static let missedIntervals: TimeInterval = 3

    private var lastLineAt: TimeInterval?

    public init() {}

    public mutating func noteLine(at seconds: TimeInterval) {
        lastLineAt = seconds
    }

    /// A new connection starts without a line.
    public mutating func reset() {
        lastLineAt = nil
    }

    public func state(at seconds: TimeInterval, intervalSeconds: TimeInterval) -> ConnectionLiveness {
        guard let last = lastLineAt else { return .unknown }
        return seconds - last > LivenessTracker.missedIntervals * intervalSeconds ? .silent : .live
    }
}

/// The completeness marks the store keeps for one project. Times are local receipt times.
public struct FreshnessMarks: Codable, Sendable, Equatable {
    /// From the first snapshot line of a connect or resync until its `snapshotEnd`.
    public var refreshing: Bool
    /// Items whose last-known value stands until their re-read: `pendingKey(section:key:)`.
    public var pending: Set<String>
    /// The receipt time of the last line, while nothing is pending and no snapshot is arriving.
    public var currentAsOf: Date?
    /// The last time the data was known to be current.
    public var lastKnownAt: Date?
    /// When a re-read failed; cleared by the next complete snapshot.
    public var refreshFailedAt: Date?

    public init(
        refreshing: Bool = false, pending: Set<String> = [], currentAsOf: Date? = nil, lastKnownAt: Date? = nil,
        refreshFailedAt: Date? = nil
    ) {
        self.refreshing = refreshing
        self.pending = pending
        self.currentAsOf = currentAsOf
        self.lastKnownAt = lastKnownAt
        self.refreshFailedAt = refreshFailedAt
    }

    public static func pendingKey(section: String, key: String) -> String {
        section + "/" + key
    }

    /// A snapshot ended: everything was read.
    mutating func complete(at now: Date) {
        refreshing = false
        pending = []
        currentAsOf = now
        lastKnownAt = now
        refreshFailedAt = nil
    }

    /// An event arrived. An event that names items to read again, or one that arrives while a snapshot is arriving,
    /// does not move `currentAsOf`.
    mutating func noteEvent(at now: Date) {
        guard !refreshing, pending.isEmpty else { return }
        currentAsOf = now
        lastKnownAt = now
    }

    /// A read failed or the stream ended before it finished. What was last known stays.
    mutating func failRefresh(at now: Date) {
        refreshFailedAt = now
    }

    /// Whether a read is outstanding.
    var isOutstanding: Bool { refreshing || !pending.isEmpty }
}

extension ProjectModel {
    mutating func markFreshness(_ change: (inout FreshnessMarks) -> Void) {
        var marks = freshness ?? FreshnessMarks()
        change(&marks)
        freshness = marks
    }

    /// What is saved of the marks: the two times that stay true after a restart. A snapshot that was arriving or an
    /// item that waited for its read is not saved; the next run reads again.
    var persistedFreshness: FreshnessMarks? {
        guard let marks = freshness, marks.lastKnownAt != nil || marks.refreshFailedAt != nil else { return nil }
        return FreshnessMarks(lastKnownAt: marks.lastKnownAt, refreshFailedAt: marks.refreshFailedAt)
    }
}

/// The three signals of one project, as the overview shows them.
public struct ProjectFreshness: Sendable, Equatable {
    public let liveness: ConnectionLiveness
    public let availability: ProjectAvailability
    public let isRefreshing: Bool
    public let pendingKeys: Set<String>
    public let currentAsOf: Date?
    public let lastKnownAt: Date?
    public let refreshFailedAt: Date?

    public init(
        liveness: ConnectionLiveness = .live, availability: ProjectAvailability = .ready, isRefreshing: Bool = false,
        pendingKeys: Set<String> = [], currentAsOf: Date? = nil, lastKnownAt: Date? = nil,
        refreshFailedAt: Date? = nil
    ) {
        self.liveness = liveness
        self.availability = availability
        self.isRefreshing = isRefreshing
        self.pendingKeys = pendingKeys
        self.currentAsOf = currentAsOf
        self.lastKnownAt = lastKnownAt
        self.refreshFailedAt = refreshFailedAt
    }

    public init(project: ProjectModel, liveness: ConnectionLiveness) {
        let marks = project.freshness ?? FreshnessMarks()
        // A project that never had a complete snapshot has nothing known, however the marks stand.
        self.init(
            liveness: liveness, availability: project.availability,
            isRefreshing: marks.refreshing || !project.snapshotComplete, pendingKeys: marks.pending,
            currentAsOf: marks.refreshing ? nil : marks.currentAsOf, lastKnownAt: marks.lastKnownAt,
            refreshFailedAt: marks.refreshFailedAt)
    }

    /// The project the app does not hold.
    public static let unknown = ProjectFreshness(isRefreshing: true)

    public var isReady: Bool { availability == .ready }

    /// The sections show last-known values: the project is not ready, the host is silent, a snapshot is arriving or
    /// the last read failed.
    public var showsLastKnown: Bool {
        !isReady || liveness == .silent || isRefreshing || refreshFailedAt != nil
    }
}
