import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class SilentDebounceTests: XCTestCase {
    let key = SilentDebounce.Key(hostId: UUID(), projectId: "p")
    let t0 = Date(timeIntervalSince1970: 1_000)

    private func at(_ s: TimeInterval) -> Date { t0.addingTimeInterval(s) }

    private func readyDebounce() -> SilentDebounce {
        var d = SilentDebounce()
        _ = d.shown(key, raw: .ready, now: t0)
        return d
    }

    func test_blinkShowsNothing() {
        var d = readyDebounce()
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(1)), .ready)
        XCTAssertEqual(d.shown(key, raw: .ready, now: at(1.8)), .ready)
    }

    func test_fiveSecondsShowsSilent() {
        var d = readyDebounce()
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(1)), .ready)
        XCTAssertEqual(d.nextDue, at(6))
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(4.9)), .ready)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(6)), .silent)
        XCTAssertEqual(d.nextDue, nil)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(60)), .silent)
    }

    func test_readyClearsAtOnce() {
        var d = readyDebounce()
        _ = d.shown(key, raw: .silent, now: at(1))
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(7)), .silent)
        XCTAssertEqual(d.shown(key, raw: .ready, now: at(7.1)), .ready)
    }

    func test_firstSeenSilentIsImmediate() {
        var d = SilentDebounce()
        XCTAssertEqual(d.shown(key, raw: .silent, now: t0), .silent)
    }

    func test_otherStatesAreImmediate() {
        var d = readyDebounce()
        XCTAssertEqual(d.shown(key, raw: .gone, now: at(1)), .gone)
        XCTAssertEqual(d.shown(key, raw: .unsupported, now: at(2)), .unsupported)
    }

    func test_shortBlinksDoNotAddUp() {
        var d = readyDebounce()
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(1)), .ready)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(4)), .ready)
        XCTAssertEqual(d.shown(key, raw: .ready, now: at(4.5)), .ready)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(5)), .ready)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(8)), .ready)
        XCTAssertEqual(d.shown(key, raw: .silent, now: at(10)), .silent)
    }
}
