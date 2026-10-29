import Foundation

/// What is kept of a host between runs. It holds text that came from the host, so the files are readable by the
/// user alone.
public struct PersistedHost: Codable, Sendable, Equatable {
    public static let currentVersion = 1

    public var version: Int
    public var hostId: UUID
    public var cursor: Cursor?
    /// Complete projects only.
    public var projects: [String: ProjectModel]
    public var forwards: [String: [ForwardRecord]]
    public var savedAt: Date

    public init(
        version: Int = PersistedHost.currentVersion, hostId: UUID, cursor: Cursor?, projects: [String: ProjectModel],
        forwards: [String: [ForwardRecord]], savedAt: Date
    ) {
        self.version = version
        self.hostId = hostId
        self.cursor = cursor
        self.projects = projects
        self.forwards = forwards
        self.savedAt = savedAt
    }

    /// The part of `host` that can be resumed: complete projects that wait for no snapshot end, and a cursor without
    /// the entries of any other project. Forwards are kept for every project, each list within its cap, because a
    /// record stays true while its project is being snapshotted again.
    init(host: HostModel, staging: Staging, savedAt: Date) {
        let projects = host.projects.filter { $0.value.snapshotComplete && staging.pending[$0.key] == nil }
        self.init(
            hostId: host.hostId, cursor: PersistedHost.trimmed(host.cursor, keeping: Set(projects.keys)),
            projects: projects,
            forwards: host.forwards.mapValues { Array($0.suffix(HostStoreEngine.maxForwardsPerProject)) },
            savedAt: savedAt)
    }

    /// The cursor without the entries of projects outside `keeping`; nil when nothing is left or the cursor is unreadable.
    static func trimmed(_ cursor: Cursor?, keeping: Set<String>) -> Cursor? {
        guard let cursor else { return nil }
        do {
            let drop = Set(try cursor.decoded().p.keys).subtracting(keeping)
            return try cursor.removing(drop)
        } catch {
            return nil
        }
    }
}

public protocol PersistenceStore: Sendable {
    /// A failed read, a damaged file and a file of another version all give nil.
    func load(hostId: UUID) -> PersistedHost?
    func save(_ host: PersistedHost) throws
    /// Removes what is kept of the host; nothing kept is not an error.
    func delete(hostId: UUID) throws
}

/// One JSON file per host: `<directory>/hosts/<uuid>.json`.
public struct FilePersistence: PersistenceStore {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var hostsDirectory: URL {
        directory.appendingPathComponent("hosts", isDirectory: true)
    }

    private func file(_ hostId: UUID) -> URL {
        hostsDirectory.appendingPathComponent("\(hostId.uuidString).json", isDirectory: false)
    }

    public func load(hostId: UUID) -> PersistedHost? {
        guard let data = try? Data(contentsOf: file(hostId)),
              let host = try? JSONDecoder().decode(PersistedHost.self, from: data),
              host.version == PersistedHost.currentVersion, host.hostId == hostId else { return nil }
        return host
    }

    public func save(_ host: PersistedHost) throws {
        let manager = FileManager.default
        try manager.createDirectory(
            at: hostsDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let target = file(host.hostId)
        try encoder.encode(host).write(to: target, options: .atomic)
        try manager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: target.path)
    }

    public func delete(hostId: UUID) throws {
        let target = file(hostId)
        guard FileManager.default.fileExists(atPath: target.path) else { return }
        try FileManager.default.removeItem(at: target)
    }
}

/// The configured hosts: `<directory>/hosts.json`.
public struct HostsFile: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var file: URL {
        directory.appendingPathComponent("hosts.json", isDirectory: false)
    }

    /// A missing or damaged file gives no hosts.
    public func load() -> [HostConfig] {
        guard let data = try? Data(contentsOf: file),
              let hosts = try? JSONDecoder().decode([HostConfig].self, from: data) else { return [] }
        return hosts
    }

    public func save(_ hosts: [HostConfig]) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(hosts).write(to: file, options: .atomic)
    }
}
