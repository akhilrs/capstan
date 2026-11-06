import Foundation
import XCTest
@testable import CapstanPhoneCore

final class SeenMergeTests: XCTestCase {
    private let hostA = UUID(uuidString: "A1A1A1A1-0000-4000-8000-000000000001")!
    private let hostB = UUID(uuidString: "B2B2B2B2-0000-4000-8000-000000000002")!

    func testEndedEpisodeKeyIsDropped() {
        let item = "\(hostA.uuidString)/proj/item"
        let ended = "pmstuck/\(hostA.uuidString)/proj/ep1"
        let next = SeenMerge.merged(previous: [item, ended], pass: [item], completeHostIds: [hostA])
        XCTAssertEqual(next, [item])
    }

    func testUnreadHostKeysAreKept() {
        let other = "\(hostB.uuidString)/proj/item"
        let otherStuck = "pmstuck/\(hostB.uuidString)/proj/ep1"
        let mine = "\(hostA.uuidString)/proj/item"
        let next = SeenMerge.merged(previous: [other, otherStuck], pass: [mine], completeHostIds: [hostA])
        XCTAssertEqual(next, [other, otherStuck, mine])
    }

    func testSecondPassWithdrawsNothing() {
        let stuck = "pmstuck/\(hostA.uuidString)/proj/ep1"
        let first = SeenMerge.merged(previous: [stuck], pass: [], completeHostIds: [hostA])
        XCTAssertTrue(first.isEmpty)
        let second = SeenMerge.merged(previous: first, pass: [], completeHostIds: [hostA])
        XCTAssertEqual(second, first)
        // The planner withdraws only keys that were stored and are gone: nothing stored, nothing to withdraw.
        XCTAssertTrue(second.filter { $0.hasPrefix(RefreshPlanner.pmStuckPrefix) }.isEmpty)
    }
}
