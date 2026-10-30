import Foundation

// The history the overview shows under Recent outcomes. It is for display only: it never goes through the reducer,
// never applies effects, never notifies and never moves the cursor. Nothing here imports the store's types of record.

/// The identity of an event across the live stream and the history read.
public struct HistoryKey: Hashable, Sendable {
    public let projectId: String
    public let epoch: String
    public let seq: Int64
    public let k: Int64

    public init(projectId: String, epoch: String, seq: Int64, k: Int64) {
        self.projectId = projectId
        self.epoch = epoch
        self.seq = seq
        self.k = k
    }
}

/// One outcome event as shown, with where it came from and when this Mac received it.
public struct HistoryEvent: Sendable, Equatable {
    public enum Source: Sendable, Equatable {
        case live, history
    }

    public let key: HistoryKey
    public let event: FeedEvent
    public let source: Source
    /// The local receipt time, for "Received (this Mac)". The host's time is `event.ts`.
    public let receivedAt: Date

    public init(key: HistoryKey, event: FeedEvent, source: Source, receivedAt: Date) {
        self.key = key
        self.event = event
        self.source = source
        self.receivedAt = receivedAt
    }
}

public enum RecentHistoryPhase: Sendable, Equatable {
    /// Nothing was asked for yet.
    case idle
    case loading
    case backfilled
    /// The host has no `recent` command; only what happened since the connection shows.
    case sessionOnly
    /// The last read failed; `message` is a sentence for the person.
    case error(String)
}

/// The history of one project.
public struct RecentHistoryState: Sendable, Equatable {
    /// The event types the host reads and the overview shows; the live ones are filtered by the same list.
    public static let outcomeTypes: Set<String> = [
        "report.recorded", "review.verdict", "verify.result", "integration.finished", "plan.approved",
        "plan.signed_off", "design.decided", "agent.lost", "agent.blocked", "finding.raised",
    ]
    /// The most events kept.
    public static let maxEvents = 100
    /// The most events a read returns, and the most the overview shows.
    public static let maxShown = 20

    public let projectId: String
    public private(set) var phase: RecentHistoryPhase = .idle
    /// The epoch and head the store holds, and sends with its read. Nil until a read begins.
    public private(set) var epoch: String?
    public private(set) var head: Int64?
    /// Newest first, by `(seq, k)`.
    public private(set) var events: [HistoryEvent] = []
    /// The host stopped at a budget before it reached the start of the ledger.
    public private(set) var truncated = false
    /// Set by a resync until a read succeeds again: "History before 14:06 may be missing".
    public private(set) var gapSince: Date?
    /// After a rate-limited answer: the earliest time a retry is sensible.
    public private(set) var retryNotBefore: Date?
    /// When this connection began, for "since you connected at 14:02".
    public private(set) var connectedAt: Date?

    public init(projectId: String) {
        self.projectId = projectId
    }

    /// The events as the overview lists them: at most `maxShown`.
    public var shown: [HistoryEvent] { Array(events.prefix(RecentHistoryState.maxShown)) }

    // MARK: Changes

    public mutating func connected(at now: Date) {
        connectedAt = now
    }

    /// A read starts at the given position. A change of epoch drops what is held.
    public mutating func begin(epoch: String, head: Int64) {
        if let held = self.epoch, held != epoch {
            events = []
            truncated = false
        }
        self.epoch = epoch
        self.head = head
        phase = .loading
    }

    /// A resync: the store is dropped and the gap is shown until the next read succeeds.
    public mutating func resync(at now: Date) {
        events = []
        truncated = false
        epoch = nil
        head = nil
        phase = .loading
        if gapSince == nil { gapSince = now }
    }

    /// The answer of a read.
    public mutating func apply(_ result: RecentResult, at now: Date) {
        switch result {
        case .page(let page):
            // An answer for another position than the one asked for (after a resync) is not shown.
            guard page.epoch == epoch, page.projectId == projectId else {
                // The read that asked for it is over; a later Refresh must be able to start another.
                if phase == .loading { phase = .idle }
                return
            }
            var kept: [HistoryEvent] = events.filter { $0.source == .live }
            let liveKeys = Set(kept.map { $0.key })
            for event in page.events where RecentHistoryState.outcomeTypes.contains(event.type) {
                let key = HistoryKey(projectId: projectId, epoch: page.epoch, seq: event.seq, k: event.k)
                // The live copy of one key wins; the history copy is dropped.
                if !liveKeys.contains(key) {
                    kept.append(HistoryEvent(key: key, event: event, source: .history, receivedAt: now))
                }
            }
            events = RecentHistoryState.ordered(kept)
            truncated = page.truncated
            phase = .backfilled
            gapSince = nil
            retryNotBefore = nil
        case .resync:
            // The ledger moved on before the read: nothing held fits it. A retry reads at the new position.
            events = []
            truncated = false
            if gapSince == nil { gapSince = now }
            phase = .error("The host's history moved on")
        case .sessionOnly:
            events = events.filter { $0.source == .live }
            truncated = false
            phase = .sessionOnly
        case .rateLimited:
            // What is held stays; the error state offers Retry after the window.
            phase = .error("The host limits how often history can be read")
            retryNotBefore = now.addingTimeInterval(RecentCall.rateLimitWindowSeconds)
        case .error:
            phase = .error("Couldn't load earlier history")
        }
    }

    /// Live outcome events join the store by the same key as the history copy; the live copy wins.
    public mutating func merge(live incoming: [FeedEvent], epoch liveEpoch: String, at now: Date) {
        if let held = epoch, held != liveEpoch {
            events = []
            truncated = false
            epoch = liveEpoch
        }
        var changed = false
        for event in incoming where RecentHistoryState.outcomeTypes.contains(event.type) {
            let key = HistoryKey(projectId: projectId, epoch: liveEpoch, seq: event.seq, k: event.k)
            let copy = HistoryEvent(key: key, event: event, source: .live, receivedAt: now)
            if let at = events.firstIndex(where: { $0.key == key }) {
                if events[at].source == .history {
                    events[at] = copy
                    changed = true
                }
            } else {
                events.append(copy)
                changed = true
            }
        }
        if changed {
            events = RecentHistoryState.ordered(events)
        }
    }

    private static func ordered(_ events: [HistoryEvent]) -> [HistoryEvent] {
        let sorted = events.sorted { (left: HistoryEvent, right: HistoryEvent) -> Bool in
            left.key.seq == right.key.seq ? left.key.k > right.key.k : left.key.seq > right.key.seq
        }
        return Array(sorted.prefix(maxEvents))
    }
}
