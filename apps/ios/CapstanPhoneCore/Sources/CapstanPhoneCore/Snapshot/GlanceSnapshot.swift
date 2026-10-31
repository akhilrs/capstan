import CapstanAppCore
import CapstanKit
import Foundation

/// A project in the glance: its display name (host text: control, bidi and line characters removed, cut to
/// `GlanceSnapshot.nameCap` bytes) and how many items wait there.
public struct GlanceProject: Codable, Sendable, Equatable {
    public let name: String
    public let count: Int

    public init(name: String, count: Int) {
        self.name = GlanceSnapshot.clean(name)
        self.count = max(0, count)
    }
}

/// What the Live Activity, the widgets and the notifications may read: counts, kinds, project names and times.
/// **It has no field for a summary, command, reason, message body or option label**, and nothing an agent wrote
/// can reach it: `make` reads only kinds, counts and project names (`GlanceSnapshotTests` enforces both).
public struct GlanceSnapshot: Codable, Sendable, Equatable {
    public static let currentVersion = 1
    /// The longest project name kept, in UTF-8 bytes.
    public static let nameCap = 64
    /// The projects kept, the busiest first.
    public static let projectCap = 8

    public let version: Int
    public let asOf: Date
    public let totalWaiting: Int
    /// Counts by the kind's wire name; read them with `count(of:)` or `byKind`.
    public let kindCounts: [String: Int]
    public let projects: [GlanceProject]
    public let hostsReachable: Int
    public let hostsTotal: Int
    /// The poll interval when the snapshot was written; staleness is measured against it.
    public let pollMs: Int

    public init(
        asOf: Date, kindCounts: [String: Int], projects: [GlanceProject], hostsReachable: Int, hostsTotal: Int,
        pollMs: Int
    ) {
        version = GlanceSnapshot.currentVersion
        self.asOf = asOf
        self.kindCounts = kindCounts.filter { $0.value > 0 }
        totalWaiting = self.kindCounts.values.reduce(0, +)
        self.projects = Array(
            projects.filter { $0.count > 0 }
                .sorted { $0.count != $1.count ? $0.count > $1.count : $0.name < $1.name }
                .prefix(GlanceSnapshot.projectCap))
        self.hostsReachable = max(0, hostsReachable)
        self.hostsTotal = max(0, hostsTotal)
        self.pollMs = max(ProtocolLimits.minPollMs, pollMs)
    }

    public func count(of kind: WaitingKind) -> Int { kindCounts[kind.wireName] ?? 0 }

    public var byKind: [WaitingKind: Int] {
        Dictionary(uniqueKeysWithValues: kindCounts.map { (WaitingKind(wire: $0.key), $0.value) })
    }

    /// The snapshot of what the hosts hold now. Only the kind and the project's display name of each waiting item
    /// are read; the item's summary is never touched.
    public static func make(
        hosts: [HostModel], hostsReachable: Int, hostsTotal: Int, pollMs: Int, asOf: Date
    ) -> GlanceSnapshot {
        var kinds: [String: Int] = [:]
        var projects: [GlanceProject] = []
        for host in hosts {
            for project in host.projects.values {
                let count = project.waiting.count
                guard count > 0 else { continue }
                for item in project.waiting.values { kinds[item.kind.wireName, default: 0] += 1 }
                projects.append(GlanceProject(name: project.displayName, count: count))
            }
        }
        return GlanceSnapshot(
            asOf: asOf, kindCounts: kinds, projects: projects, hostsReachable: hostsReachable,
            hostsTotal: hostsTotal, pollMs: pollMs)
    }

    /// Control, bidi and line characters become spaces (the kit's display rules), runs of spaces close up, and the
    /// text is cut to `nameCap` bytes between characters.
    static func clean(_ text: String) -> String {
        let plain = UntrustedText(raw: text, verified: true, truncated: false).displayPlain
        let single = plain.split(whereSeparator: { $0 == " " }).joined(separator: " ")
        return TextBudget.displayPrefix(single, maxBytes: nameCap).text
    }
}

/// Whether a glance is live or shows "as of 12:31": stale once it is older than two poll intervals and a grace.
public enum GlanceStaleness: Equatable, Sendable {
    case live
    case stale(String)

    public static let graceSeconds: TimeInterval = 10

    public static func label(asOf: Date, now: Date, pollMs: Int, timeZone: TimeZone = .current) -> GlanceStaleness {
        let limit = 2 * Double(max(pollMs, ProtocolLimits.minPollMs)) / 1000 + graceSeconds
        guard now.timeIntervalSince(asOf) > limit else { return .live }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let parts = calendar.dateComponents([.hour, .minute], from: asOf)
        let time = String(format: "%02d:%02d", parts.hour ?? 0, parts.minute ?? 0)
        return .stale("as of " + time)
    }

    public static func label(_ snapshot: GlanceSnapshot, now: Date, timeZone: TimeZone = .current) -> GlanceStaleness {
        label(asOf: snapshot.asOf, now: now, pollMs: snapshot.pollMs, timeZone: timeZone)
    }
}

/// `glance.json` in a directory both the app and its extension can read (the App Group container in the app).
public struct GlanceStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    public var file: URL { directory.appendingPathComponent("glance.json", isDirectory: false) }

    /// Writes atomically: a reader sees the old file or the new one, never half of one.
    public func write(_ snapshot: GlanceSnapshot) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .secondsSince1970
        var options: Data.WritingOptions = [.atomic]
        #if os(iOS)
        // Readable by the widget after the first unlock, not before.
        options.insert(.completeFileProtectionUntilFirstUserAuthentication)
        #endif
        try encoder.encode(snapshot).write(to: file, options: options)
    }

    /// The last snapshot; nil when there is none, it cannot be read, or it is of another version.
    public func read() -> GlanceSnapshot? {
        guard let data = try? Data(contentsOf: file) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        guard let snapshot = try? decoder.decode(GlanceSnapshot.self, from: data),
            snapshot.version == GlanceSnapshot.currentVersion
        else { return nil }
        return snapshot
    }
}
