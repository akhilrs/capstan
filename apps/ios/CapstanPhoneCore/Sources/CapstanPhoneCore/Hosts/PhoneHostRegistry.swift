import Foundation

/// The hosts the transport can reach, by alias. Readable from any thread or event loop.
///
/// Saved hosts are registered at launch. A host being checked is a candidate: `commit` keeps it when it is added,
/// `discard` drops it and forgets its pins. Candidates are memory only, so they vanish on relaunch (and
/// `HostKeyStore.sweep` removes their pins). A `resolve` returns a copy, so a connection that resolved before a
/// `discard` keeps its host for that one connection.
public final class PhoneHostRegistry: @unchecked Sendable {
    private let store: HostKeyStore
    private let lock = NSLock()
    private var saved: [UUID: PhoneHost] = [:]
    private var candidates: [UUID: PhoneHost] = [:]
    private var byAlias: [String: UUID] = [:]

    public init(store: HostKeyStore) {
        self.store = store
    }

    /// The host for an alias, or nil (the transport then refuses the destination).
    public func resolve(_ alias: String) -> PhoneHost? {
        lock.withLock {
            guard let id = byAlias[alias] else { return nil }
            return saved[id] ?? candidates[id]
        }
    }

    public func host(_ id: UUID) -> PhoneHost? {
        lock.withLock { saved[id] ?? candidates[id] }
    }

    /// The saved hosts.
    public var savedHosts: [PhoneHost] {
        lock.withLock { Array(saved.values) }
    }

    /// A fresh id whose alias is not registered.
    public func newHostId() -> UUID {
        lock.withLock {
            while true {
                let id = UUID()
                if byAlias[PhoneHost.alias(for: id)] == nil { return id }
            }
        }
    }

    /// Registers a saved host (at launch).
    public func register(_ host: PhoneHost) throws {
        try lock.withLock {
            try check(host)
            saved[host.id] = host
            byAlias[host.alias] = host.id
        }
    }

    /// Registers a host that is being checked before it is added.
    public func registerCandidate(_ host: PhoneHost) throws {
        try lock.withLock {
            try check(host)
            candidates[host.id] = host
            byAlias[host.alias] = host.id
        }
    }

    /// Keeps a candidate: the host was added.
    public func commit(_ id: UUID) {
        lock.withLock {
            guard let host = candidates.removeValue(forKey: id) else { return }
            saved[id] = host
        }
    }

    /// Drops a candidate that was not added, and forgets its pins.
    public func discard(_ id: UUID) {
        let dropped: Bool = lock.withLock {
            guard candidates.removeValue(forKey: id) != nil else { return false }
            byAlias.removeValue(forKey: PhoneHost.alias(for: id))
            return true
        }
        if dropped { store.forget(hostId: id) }
    }

    /// Removes a saved host (Remove host), and forgets its pins.
    public func remove(_ id: UUID) {
        let removed: Bool = lock.withLock {
            guard saved.removeValue(forKey: id) != nil else { return false }
            byAlias.removeValue(forKey: PhoneHost.alias(for: id))
            return true
        }
        if removed { store.forget(hostId: id) }
    }

    /// Called with `lock` held.
    private func check(_ host: PhoneHost) throws {
        if let error = host.validate().first { throw error }
        if let owner = byAlias[host.alias], owner != host.id { throw PhoneHostError.aliasInUse }
        let identity = host.identity
        if saved.values.contains(where: { $0.id != host.id && $0.identity == identity }) {
            throw PhoneHostError.duplicate
        }
    }
}
