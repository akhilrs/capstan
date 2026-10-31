import Foundation
import XCTest
@testable import CapstanPhoneCore

final class WaitingActivityStateTests: XCTestCase {
    private func state(_ kinds: [String: Int], live: Int = 1, total: Int = 1) -> WaitingActivityState {
        WaitingActivityState(GlanceSnapshot(
            asOf: Date(timeIntervalSince1970: 0), kindCounts: kinds, projects: [], hostsReachable: live,
            hostsTotal: total, pollMs: 2000))
    }

    func testKindsLine() {
        let s = state(["op": 2, "prompt": 2, "design": 1, "message": 1, "integration": 1])
        XCTAssertEqual(s.total, 7)
        XCTAssertEqual(s.kindsLine, "2 proposals · 2 prompts · 1 design · 1 message · 1 merge")
        XCTAssertEqual(s.shortLine(), "2 proposals · 2 prompts · 3 more")
    }

    func testZeroKindsAreLeftOut() {
        let s = state(["prompt": 1])
        XCTAssertEqual(s.kindsLine, "1 prompt")
        XCTAssertEqual(s.shortLine(), "1 prompt")
    }

    func testHostsLine() {
        XCTAssertEqual(state(["op": 1]).hostsLine, "live")
        XCTAssertEqual(state(["op": 1], live: 0, total: 1).hostsLine, "not live")
        XCTAssertEqual(state(["op": 1], live: 1, total: 2).hostsLine, "1 of 2 hosts live")
    }

    func testTheStateHasNoTextField() {
        let forbidden = ["summary", "command", "reason", "body", "option", "text", "name", "task"]
        for field in Mirror(reflecting: state(["op": 1])).children.compactMap(\.label) {
            for word in forbidden {
                XCTAssertFalse(field.lowercased().contains(word), "field \(field) looks like it carries text")
            }
        }
    }
}
