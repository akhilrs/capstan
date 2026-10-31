import Foundation
import XCTest
@testable import CapstanPhoneCore

final class GateTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    private func required(
        settingOn: Bool = true, lastUnlock: Date?, backgroundedAt: Date?, now: Date
    ) -> Bool {
        FaceIDRule.required(settingOn: settingOn, lastUnlock: lastUnlock, backgroundedAt: backgroundedAt, now: now)
    }

    func testThresholdIsSixtySeconds() {
        XCTAssertEqual(FaceIDRule.awayThreshold, 60)
    }

    func testFiftyNineSecondsAwayIsNotRequired() {
        let away = start.addingTimeInterval(10)
        XCTAssertFalse(required(lastUnlock: start, backgroundedAt: away, now: away.addingTimeInterval(59)))
    }

    func testSixtySecondsAwayIsNotRequired() {
        let away = start.addingTimeInterval(10)
        XCTAssertFalse(required(lastUnlock: start, backgroundedAt: away, now: away.addingTimeInterval(60)))
    }

    func testSixtyOneSecondsAwayIsRequired() {
        let away = start.addingTimeInterval(10)
        XCTAssertTrue(required(lastUnlock: start, backgroundedAt: away, now: away.addingTimeInterval(61)))
    }

    func testNeverUnlockedIsRequired() {
        XCTAssertTrue(required(lastUnlock: nil, backgroundedAt: start, now: start.addingTimeInterval(5)))
    }

    func testColdStartIsRequired() {
        XCTAssertTrue(required(lastUnlock: nil, backgroundedAt: nil, now: start))
    }

    func testNeverBackgroundedIsNotRequiredOnceUnlocked() {
        XCTAssertFalse(required(lastUnlock: start, backgroundedAt: nil, now: start.addingTimeInterval(3600)))
    }

    func testSettingOffIsNeverRequired() {
        XCTAssertFalse(required(settingOn: false, lastUnlock: nil, backgroundedAt: nil, now: start))
        XCTAssertFalse(required(
            settingOn: false, lastUnlock: start, backgroundedAt: start, now: start.addingTimeInterval(3600)))
    }

    /// The app's bookkeeping from plan section 1.2: `.background` sets `backgroundedAt`; an `.active` check that passed
    /// or was not required sets `lastUnlock` (if passed) and clears `backgroundedAt`.
    func testTwoBackgroundCycles() {
        var lastUnlock: Date?
        var backgroundedAt: Date?

        // Cold start: required, and the person unlocks.
        var now = start
        XCTAssertTrue(required(lastUnlock: lastUnlock, backgroundedAt: backgroundedAt, now: now))
        lastUnlock = now
        backgroundedAt = nil

        // Cycle 1: 30 s away, not required.
        now = now.addingTimeInterval(100)
        backgroundedAt = now
        now = now.addingTimeInterval(30)
        XCTAssertFalse(required(lastUnlock: lastUnlock, backgroundedAt: backgroundedAt, now: now))
        backgroundedAt = nil

        // Cycle 2: 90 s away, required, even though the last unlock was long ago in both cycles.
        now = now.addingTimeInterval(100)
        backgroundedAt = now
        now = now.addingTimeInterval(90)
        XCTAssertTrue(required(lastUnlock: lastUnlock, backgroundedAt: backgroundedAt, now: now))
        lastUnlock = now
        backgroundedAt = nil

        // Back in the app after the unlock: not required.
        XCTAssertFalse(required(lastUnlock: lastUnlock, backgroundedAt: backgroundedAt, now: now.addingTimeInterval(5)))
    }
}
