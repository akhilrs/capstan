import Foundation
import XCTest
@testable import CapstanPhoneCore

final class LifecyclePlannerTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    /// A state after a cold start whose Face ID passed at `start`, sessions running.
    private func unlockedState() -> LifecycleState {
        LifecyclePlanner.unlocked(state: LifecycleState(), now: start).state
    }

    func testColdStartWithTheSettingOnPrompts() {
        let plan = LifecyclePlanner.phase(.active, state: LifecycleState(), faceIDOn: true, now: start)
        XCTAssertEqual(plan.actions, [.lockUI, .promptFaceID])
        XCTAssertTrue(plan.state.locked)
        XCTAssertFalse(plan.state.sessionsRunning, "sessions wait for the unlock")

        let unlocked = LifecyclePlanner.unlocked(state: plan.state, now: start)
        XCTAssertEqual(unlocked.actions, [.unlockUI, .startSessions])
        XCTAssertEqual(unlocked.state.lastUnlock, start)
        XCTAssertFalse(unlocked.state.locked)
    }

    func testFiftyNineSecondsAwayDoesNotPrompt() {
        let away = LifecyclePlanner.phase(.background, state: unlockedState(), faceIDOn: true, now: start.addingTimeInterval(10))
        let back = LifecyclePlanner.phase(.active, state: away.state, faceIDOn: true, now: start.addingTimeInterval(69))
        XCTAssertEqual(back.actions, [.startSessions])
        XCTAssertNil(back.state.backgroundedAt, "a check that was not required clears it")
    }

    func testSixtyOneSecondsAwayPrompts() {
        let away = LifecyclePlanner.phase(.background, state: unlockedState(), faceIDOn: true, now: start.addingTimeInterval(10))
        let back = LifecyclePlanner.phase(.active, state: away.state, faceIDOn: true, now: start.addingTimeInterval(71))
        XCTAssertEqual(back.actions, [.lockUI, .promptFaceID])
    }

    func testACancelledUnlockIsNotUndoneByComingBackWithinTheMinute() {
        // Unlocked once, then away over a minute: the cover goes up and the person cancels Face ID.
        let away = LifecyclePlanner.phase(.background, state: unlockedState(), faceIDOn: true, now: start.addingTimeInterval(10))
        let back = LifecyclePlanner.phase(.active, state: away.state, faceIDOn: true, now: start.addingTimeInterval(100))
        let cancelled = LifecyclePlanner.unlockFailed(state: back.state)
        // Home Screen for ten seconds and back: still covered, Face ID asked again, no session started.
        let home = LifecyclePlanner.phase(.background, state: cancelled.state, faceIDOn: true, now: start.addingTimeInterval(110))
        let again = LifecyclePlanner.phase(.active, state: home.state, faceIDOn: true, now: start.addingTimeInterval(120))
        XCTAssertEqual(again.actions, [.lockUI, .promptFaceID])
        XCTAssertTrue(again.state.locked)
        XCTAssertFalse(again.actions.contains(.startSessions))
    }

    func testACancelAsksOnceNotInALoop() {
        // Cold start asks; the Face ID sheet closing makes the scene active again: the cover stays, no second prompt.
        let cold = LifecyclePlanner.phase(.active, state: LifecycleState(locked: true), faceIDOn: true, now: start)
        XCTAssertEqual(cold.actions, [.lockUI, .promptFaceID])
        let cancelled = LifecyclePlanner.unlockFailed(state: cold.state)
        let sheetGone = LifecyclePlanner.phase(.active, state: cancelled.state, faceIDOn: true, now: start.addingTimeInterval(2))
        XCTAssertEqual(sheetGone.actions, [.lockUI])
        // Leaving and coming back asks again.
        let home = LifecyclePlanner.phase(.background, state: sheetGone.state, faceIDOn: true, now: start.addingTimeInterval(5))
        let back = LifecyclePlanner.phase(.active, state: home.state, faceIDOn: true, now: start.addingTimeInterval(8))
        XCTAssertEqual(back.actions, [.lockUI, .promptFaceID])
    }

    func testInactiveRecordsNothing() {
        let state = unlockedState()
        let plan = LifecyclePlanner.phase(.inactive, state: state, faceIDOn: true, now: start.addingTimeInterval(500))
        XCTAssertEqual(plan.actions, [.noop])
        XCTAssertEqual(plan.state, state)
        // Back from the Face ID sheet or the notification shade: nothing to do.
        let back = LifecyclePlanner.phase(.active, state: plan.state, faceIDOn: true, now: start.addingTimeInterval(600))
        XCTAssertEqual(back.actions, [.noop])
    }

    func testBackgroundStopsSessionsThenSuspends() {
        let plan = LifecyclePlanner.phase(.background, state: unlockedState(), faceIDOn: true, now: start)
        XCTAssertEqual(plan.actions, [.stopSessionsThenSuspend], "one action: stop, then suspend, in that order")
        XCTAssertEqual(plan.state.backgroundedAt, start)
        XCTAssertFalse(plan.state.sessionsRunning)
    }

    func testSettingOffNeverPrompts() {
        let cold = LifecyclePlanner.phase(.active, state: LifecycleState(), faceIDOn: false, now: start)
        XCTAssertEqual(cold.actions, [.startSessions])
        let away = LifecyclePlanner.phase(.background, state: cold.state, faceIDOn: false, now: start)
        let back = LifecyclePlanner.phase(.active, state: away.state, faceIDOn: false, now: start.addingTimeInterval(86_400))
        XCTAssertEqual(back.actions, [.startSessions])
    }

    func testBackgroundRefreshNeverPrompts() {
        // Never unlocked, long away, setting on: the most a foreground would ask for.
        let state = LifecycleState(lastUnlock: nil, backgroundedAt: start, locked: true)
        let plan = LifecyclePlanner.backgroundRefresh(state: state, now: start.addingTimeInterval(86_400))
        XCTAssertFalse(plan.actions.contains(.promptFaceID))
        XCTAssertEqual(plan.state, state)
    }

    func testAFailedUnlockKeepsTheCover() {
        let locked = LifecyclePlanner.phase(.active, state: LifecycleState(), faceIDOn: true, now: start)
        let failed = LifecyclePlanner.unlockFailed(state: locked.state)
        XCTAssertEqual(failed.actions, [.lockUI])
        XCTAssertTrue(failed.state.locked)
        XCTAssertNil(failed.state.lastUnlock)
    }

    func testLockedCoverOpensWhenTheSettingIsSwitchedOff() {
        let locked = LifecyclePlanner.phase(.active, state: LifecycleState(), faceIDOn: true, now: start)
        let back = LifecyclePlanner.phase(.active, state: locked.state, faceIDOn: false, now: start)
        XCTAssertEqual(back.actions, [.unlockUI, .startSessions])
    }
}
