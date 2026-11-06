import Foundation

/// What the app does with its host sessions.
public enum SessionAction: Sendable, Equatable {
    case none
    case start
    case stop
}

/// Counts the voice runs that hold the host sessions open. Pure: the app passes in whether it is in the background.
/// `begin` always says start (starting twice is harmless); `end` says stop only for the last holder, and only while
/// the app is in the background, because a foreground app keeps its sessions.
public struct SessionLease: Sendable, Equatable {
    public private(set) var holders = 0

    public init() {}

    public mutating func begin() -> SessionAction {
        holders += 1
        return .start
    }

    public mutating func end(isBackground: Bool) -> SessionAction {
        holders = max(0, holders - 1)
        return holders == 0 && isBackground ? .stop : .none
    }
}

/// A `SessionLease` shared by the runs of one process.
public final class SessionLeaseBox: @unchecked Sendable {
    private let lock = NSLock()
    private var lease = SessionLease()

    public init() {}

    private func begin() -> SessionAction {
        lock.lock()
        defer { lock.unlock() }
        return lease.begin()
    }

    private func end(isBackground: Bool) -> SessionAction {
        lock.lock()
        defer { lock.unlock() }
        return lease.end(isBackground: isBackground)
    }

    /// Runs `body` with the lease held: `start` runs when it begins, `stop` when it ends (also when `body` throws)
    /// and the lease says stop.
    public func hold<T: Sendable>(
        isBackground: @Sendable () -> Bool, start: @Sendable () async -> Void, stop: @Sendable () async -> Void,
        body: @Sendable () async throws -> T
    ) async throws -> T {
        if begin() == .start { await start() }
        do {
            let value = try await body()
            if end(isBackground: isBackground()) == .stop { await stop() }
            return value
        } catch {
            if end(isBackground: isBackground()) == .stop { await stop() }
            throw error
        }
    }
}
