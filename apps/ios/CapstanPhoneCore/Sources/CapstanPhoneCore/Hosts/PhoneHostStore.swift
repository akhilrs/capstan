import Foundation

/// The phone hosts in `phone-hosts.json`, beside `HostsFile`'s `hosts.json` in Application Support.
public struct PhoneHostStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var file: URL {
        directory.appendingPathComponent("phone-hosts.json", isDirectory: false)
    }

    /// A missing or damaged file gives no hosts; a damaged file is left as it is until the next `save`.
    public func load() -> [PhoneHost] {
        guard let data = try? Data(contentsOf: file),
              let hosts = try? JSONDecoder().decode([PhoneHost].self, from: data) else { return [] }
        return hosts
    }

    public func save(_ hosts: [PhoneHost]) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(hosts).write(to: file, options: .atomic)
    }
}
