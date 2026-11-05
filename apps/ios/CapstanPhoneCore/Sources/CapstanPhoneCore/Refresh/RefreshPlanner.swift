import CapstanAppCore
import CapstanKit
import Foundation

/// The rules of a refresh (plan 7.4), pure: which hosts to read, when to stop, which notifications to post, and the
/// items seen so far. The same rules run in the foreground after every model change and in a background refresh, so
/// an item is announced once, whichever saw it first. **It has no `BiometricGate` and no `Decider`:** a refresh
/// never asks for Face ID and never sends anything to a host but the read-only stream.
public enum RefreshPlanner {
    /// iOS gives a background refresh about 30 s; the work stops before that.
    public static let budgetSeconds: TimeInterval = 25
    /// At most this many notifications in one pass; the rest are counted in the badge and the Live Activity.
    public static let notificationCap = 8

    /// The hosts a refresh reads: enabled, and not blocked by a problem the person must fix first.
    public static func hosts(_ hosts: [HostConfig], blocked: Set<UUID>) -> [UUID] {
        hosts.filter { $0.enabled && !blocked.contains($0.id) }.map(\.id)
    }

    /// Stop when the budget is spent, or once every host read has a complete snapshot of its ready projects.
    public static func shouldStop(started: Date, now: Date, complete: Bool) -> Bool {
        complete || now.timeIntervalSince(started) >= budgetSeconds
    }

    /// True when a host's model holds a complete snapshot of every ready project (at least one project read).
    public static func isComplete(_ model: HostModel?) -> Bool {
        guard let model, !model.projects.isEmpty else { return false }
        return model.projects.values.allSatisfy { $0.availability != .ready || $0.snapshotComplete }
    }

    /// The key an item is remembered by: host, project and request id (a new request on the same key is new).
    public static func seenKey(hostId: UUID, projectId: String, requestId: String) -> String {
        hostId.uuidString + "/" + projectId + "/" + requestId
    }

    public struct Pass: Equatable, Sendable {
        public let notifications: [PhoneNotificationPlan]
        /// What is waiting now: the next pass compares against it.
        public let seen: Set<String>
        /// Identifiers of notifications to withdraw: stuck-PM episodes that have ended.
        public let withdraw: [String]

        public init(notifications: [PhoneNotificationPlan], seen: Set<String>, withdraw: [String] = []) {
            self.notifications = notifications
            self.seen = seen
            self.withdraw = withdraw
        }
    }

    /// Prefix of the seen keys of stuck-PM episodes.
    static let pmStuckPrefix = "pmstuck/"

    /// The notification identifier of a `pmstuck/<host>/<project>/<episode>` key; nil for any other key.
    static func pmStuckIdentifier(ofSeenKey key: String) -> String? {
        guard key.hasPrefix(pmStuckPrefix) else { return nil }
        let parts = key.dropFirst(pmStuckPrefix.count).split(separator: "/", maxSplits: 2, omittingEmptySubsequences: false)
        guard parts.count == 3, let host = UUID(uuidString: String(parts[0])) else { return nil }
        return NotificationPlan.pmStuckIdentifier(hostId: host, projectId: String(parts[1]), episodeId: String(parts[2]))
    }

    /// The notifications for items that wait now and were not seen before, by kind and project name only, at most
    /// `notificationCap`, oldest first. `seen` nil is the first pass ever: everything waiting becomes the baseline and
    /// nothing is announced. Projects that are not ready are left out (their items cannot be answered).
    public static func pass(
        models: [HostModel], seen: Set<String>?, settings: PhoneSettings, fileCheck: (String) -> Bool = { _ in true }
    ) -> Pass {
        var now: Set<String> = []
        var fresh: [(since: String, plan: PhoneNotificationPlan)] = []
        for model in models {
            for project in model.projects.values where project.availability == .ready {
                if let stuck = project.run?.pm?.stuck, stuck.alert, project.snapshotComplete {
                    let key = pmStuckPrefix + seenKey(hostId: model.hostId, projectId: project.projectId, requestId: stuck.episodeId)
                    now.insert(key)
                    if let seen, !seen.contains(key),
                       let plan = PhoneNotificationPlan(
                           pmStuck: model.hostId, projectId: project.projectId, episodeId: stuck.episodeId,
                           projectName: project.displayName, settings: settings, fileCheck: fileCheck)
                    {
                        fresh.append((stuck.since ?? "", plan))
                    }
                }
                for item in project.waiting.values {
                    let key = seenKey(hostId: model.hostId, projectId: project.projectId, requestId: item.requestId)
                    now.insert(key)
                    guard let seen, !seen.contains(key) else { continue }
                    let candidate = NotifyCandidate(
                        hostId: model.hostId, projectId: project.projectId, kind: item.kind, key: item.key)
                    if let plan = PhoneNotificationPlan(
                        candidate: candidate, requestId: item.requestId, projectName: project.displayName,
                        settings: settings, fileCheck: fileCheck)
                    {
                        fresh.append((item.since, plan))
                    }
                }
            }
        }
        // A stuck episode of a host read in this pass that is gone ended: its notification is withdrawn. A project that
        // cannot be read right now (not ready, no complete snapshot) keeps what it had.
        var withdraw: [String] = []
        if let seen {
            let read = Dictionary(uniqueKeysWithValues: models.map { ($0.hostId, $0) })
            for key in seen.sorted() where key.hasPrefix(pmStuckPrefix) && !now.contains(key) {
                let parts = key.dropFirst(pmStuckPrefix.count).split(separator: "/", maxSplits: 2, omittingEmptySubsequences: false)
                guard parts.count == 3, let host = UUID(uuidString: String(parts[0])), let model = read[host],
                      let identifier = pmStuckIdentifier(ofSeenKey: key) else { continue }
                if let project = model.projects[String(parts[1])], !(project.availability == .ready && project.snapshotComplete) {
                    now.insert(key)
                } else {
                    withdraw.append(identifier)
                }
            }
        }
        let ordered = fresh.sorted { $0.since != $1.since ? $0.since < $1.since : $0.plan.identifier < $1.plan.identifier }
        return Pass(notifications: ordered.prefix(notificationCap).map(\.plan), seen: now, withdraw: withdraw)
    }
}

/// `seen.json`: the waiting items already announced. Missing means the first pass.
public struct SeenStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var file: URL { directory.appendingPathComponent("seen.json", isDirectory: false) }

    public func read() -> Set<String>? {
        guard let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(Set<String>.self, from: data)
    }

    public func write(_ seen: Set<String>) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try JSONEncoder().encode(seen).write(to: file, options: .atomic)
    }
}
