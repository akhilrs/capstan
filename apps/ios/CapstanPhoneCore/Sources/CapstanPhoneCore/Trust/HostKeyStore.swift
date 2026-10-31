import Foundation

/// The host-key pins, one per host id. Verdicts come from memory with no IO, so they are safe on an event loop;
/// writes go to `host-keys.json` on a serial queue of their own, in the order the changes were made.
///
/// A pin file that exists but cannot be read makes every verdict `pinsUnreadable` and is left untouched until
/// `resetAll()`.
public final class HostKeyStore: @unchecked Sendable {
    private let file: URL
    private let lock = NSLock()
    private let writes = DispatchQueue(label: "capstan.host-key-store")
    private var pins: [UUID: HostKeyPin] = [:]
    private var unreadable = false

    public init(directory: URL) {
        file = directory.appendingPathComponent("host-keys.json", isDirectory: false)
        if FileManager.default.fileExists(atPath: file.path) {
            if let data = try? Data(contentsOf: file), let loaded = try? JSONDecoder().decode([UUID: HostKeyPin].self, from: data) {
                pins = loaded
            } else {
                unreadable = true
            }
        }
    }

    public var isUnreadable: Bool {
        lock.withLock { unreadable }
    }

    public func pin(for hostId: UUID) -> HostKeyPin? {
        lock.withLock { pins[hostId] }
    }

    public func verdict(hostId: UUID, offered: HostKeyPin) -> TrustVerdict {
        lock.withLock {
            if unreadable { return .pinsUnreadable }
            guard let pinned = pins[hostId] else { return .unknown(offered) }
            return pinned == offered ? .trusted : .changed(pinned: pinned, offered: offered)
        }
    }

    /// Pins `pin` for the host only when it has no pin (and the file is readable). True when it was pinned.
    /// Returns once the write is queued.
    @discardableResult
    public func trust(hostId: UUID, pin: HostKeyPin) -> Bool {
        lock.withLock {
            guard !unreadable, pins[hostId] == nil else { return false }
            pins[hostId] = pin
            queueWrite()
            return true
        }
    }

    /// Clears the host's pin (Remove host, or a candidate that was not added).
    public func forget(hostId: UUID) {
        lock.withLock {
            guard !unreadable, pins.removeValue(forKey: hostId) != nil else { return }
            queueWrite()
        }
    }

    /// Drops the pins of hosts that are not saved, at launch.
    public func sweep(keeping ids: Set<UUID>) {
        lock.withLock {
            guard !unreadable else { return }
            let before = pins.count
            pins = pins.filter { ids.contains($0.key) }
            if pins.count != before { queueWrite() }
        }
    }

    /// Forgets every pin and replaces an unreadable file; every host then asks for trust again.
    public func resetAll() {
        lock.withLock {
            pins = [:]
            unreadable = false
            queueWrite()
        }
    }

    /// Waits until every queued write has landed. For tests and for a clean exit.
    public func waitForWrites() {
        writes.sync {}
    }

    /// Called with `lock` held, so the writes keep the order of the changes.
    private func queueWrite() {
        let snapshot = pins
        let file = self.file
        writes.async {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.sortedKeys]
            guard let data = try? encoder.encode(snapshot) else { return }
            try? FileManager.default.createDirectory(
                at: file.deletingLastPathComponent(), withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            try? data.write(to: file, options: .atomic)
        }
    }
}
