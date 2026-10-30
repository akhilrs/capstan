import CapstanKit
import Foundation

/// Holds back a project's brief `silent` state. The host reports silent after a few missed polls and ready again at the
/// first answer, so a short stall of the daemon makes a blink; the app shows silent only after the project has been
/// silent without a break for `delay`. Ready, and every other state, is shown at once. A project that was never seen
/// ready since launch shows silent at once, as the host does. Pure: the clock is passed in.
public struct SilentDebounce: Sendable {
    public struct Key: Hashable, Sendable {
        public let hostId: UUID
        public let projectId: String

        public init(hostId: UUID, projectId: String) {
            self.hostId = hostId
            self.projectId = projectId
        }
    }

    /// Seconds a project must stay silent before silent is shown.
    public static let delay: TimeInterval = 5

    private var seenReady: Set<Key> = []
    private var silentSince: [Key: Date] = [:]
    private var lastShown: [Key: ProjectAvailability] = [:]

    public init() {}

    /// The state to show for a project whose host-reported state is `raw` at `now`.
    public mutating func shown(_ key: Key, raw: ProjectAvailability, now: Date) -> ProjectAvailability {
        guard raw == .silent else {
            silentSince[key] = nil
            if raw == .ready { seenReady.insert(key) }
            lastShown[key] = raw
            return raw
        }
        guard seenReady.contains(key) else {
            lastShown[key] = raw
            return raw
        }
        let since = silentSince[key] ?? now
        silentSince[key] = since
        if now.timeIntervalSince(since) >= Self.delay {
            lastShown[key] = raw
            return raw
        }
        return lastShown[key] ?? .ready
    }

    /// When a held-back silent state is due to show, the earliest of them; nil when none is waiting.
    public var nextDue: Date? {
        silentSince.compactMap { key, since in
            lastShown[key] == .silent ? nil : since.addingTimeInterval(Self.delay)
        }.min()
    }

    /// Forgets projects that are not in `keys` any more.
    public mutating func retain(_ keys: Set<Key>) {
        seenReady.formIntersection(keys)
        silentSince = silentSince.filter { keys.contains($0.key) }
        lastShown = lastShown.filter { keys.contains($0.key) }
    }
}
