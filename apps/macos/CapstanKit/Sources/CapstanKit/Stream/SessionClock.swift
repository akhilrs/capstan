import Foundation

/// The time source of a host session, so tests can run without waiting.
public protocol SessionClock: Sendable {
    func now() -> Date
    /// Returns after the given seconds; throws `CancellationError` when the calling task is cancelled.
    func sleep(seconds: TimeInterval) async throws
}

public struct SystemClock: SessionClock {
    public init() {}

    public func now() -> Date {
        Date()
    }

    public func sleep(seconds: TimeInterval) async throws {
        guard seconds > 0 else { return }
        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }
}
