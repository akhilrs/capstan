import Foundation
import XCTest
@testable import CapstanKit

/// A clock that moves only when a test says so.
final class TestClock: SessionClock, @unchecked Sendable {
    private struct Sleeper {
        let wake: Date
        let continuation: CheckedContinuation<Void, Error>
    }

    private let lock = NSLock()
    private var current: Date
    private var sleepers: [UUID: Sleeper] = [:]
    private var cancelled: Set<UUID> = []

    init(start: Date = Date(timeIntervalSince1970: 1_000_000)) {
        current = start
    }

    func now() -> Date {
        lock.withLock { current }
    }

    func sleep(seconds: TimeInterval) async throws {
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                let outcome: Bool = lock.withLock {
                    if cancelled.remove(id) != nil || Task.isCancelled { return false }
                    sleepers[id] = Sleeper(wake: current.addingTimeInterval(seconds), continuation: continuation)
                    return true
                }
                if !outcome { continuation.resume(throwing: CancellationError()) }
            }
        } onCancel: {
            let sleeper: Sleeper? = lock.withLock {
                if let found = sleepers.removeValue(forKey: id) { return found }
                cancelled.insert(id)
                return nil
            }
            sleeper?.continuation.resume(throwing: CancellationError())
        }
    }

    /// Moves time forward and wakes the sleepers that are due, earliest first.
    func advance(by seconds: TimeInterval) {
        let due: [Sleeper] = lock.withLock {
            current = current.addingTimeInterval(seconds)
            let ready = sleepers.filter { $0.value.wake <= current }
            for id in ready.keys { sleepers.removeValue(forKey: id) }
            return ready.values.sorted { $0.wake < $1.wake }
        }
        for sleeper in due { sleeper.continuation.resume() }
    }

    var sleeperCount: Int {
        lock.withLock { sleepers.count }
    }
}

/// Polls a condition with real sleeps until it holds, for tests that wait on tasks.
func eventually(
    timeout: TimeInterval = 3, _ message: String = "condition", file: StaticString = #filePath, line: UInt = #line,
    _ condition: () async -> Bool
) async {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if await condition() { return }
        try? await Task.sleep(nanoseconds: 5_000_000)
    }
    XCTFail("timed out waiting for \(message)", file: file, line: line)
}

/// Lets queued tasks run; use after `advance` when nothing observable is expected to change.
func settle() async {
    try? await Task.sleep(nanoseconds: 60_000_000)
}
