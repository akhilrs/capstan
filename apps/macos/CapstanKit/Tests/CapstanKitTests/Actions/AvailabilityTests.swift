import XCTest
@testable import CapstanKit

final class AvailabilityTests: XCTestCase {
    private let yes = HostCapabilities(reachable: true, events: true, decide: true)
    private let no = HostCapabilities(reachable: true, events: true, decide: false)

    func testNoDecideCapabilityHidesEverything() throws {
        let verbs: [DecideVerb] = [.opApprove, .promptEsc, .promptOption(1), .agentPause(reason: nil), .runResume(reason: nil)]
        for verb in verbs {
            guard case .hidden = availability(verb, capabilities: no, item: nil, option: promptOption()) else {
                return XCTFail("\(verb)")
            }
        }
    }

    func testPlainOptionIsAvailable() throws {
        XCTAssertEqual(
            availability(.promptOption(1), capabilities: yes, item: try waitingItem(), option: promptOption()), .available)
    }

    func testWidensPermissionsIsTerminalOnly() throws {
        guard case .terminalOnly = availability(
            .promptOption(1), capabilities: yes, item: try waitingItem(), option: promptOption(widens: true))
        else { return XCTFail() }
    }

    func testAcceptsTextIsTerminalOnly() throws {
        guard case .terminalOnly = availability(
            .promptOption(1), capabilities: yes, item: try waitingItem(), option: promptOption(accepts: true))
        else { return XCTFail() }
    }

    func testNeedsTerminalAllowsOnlyEsc() throws {
        let item = try waitingItem(needsTerminal: true)
        guard case .terminalOnly = availability(.promptOption(1), capabilities: yes, item: item, option: promptOption())
        else { return XCTFail() }
        XCTAssertEqual(availability(.promptEsc, capabilities: yes, item: item, option: nil), .available)
    }

    func testOptionVerbWithoutOptionIsHidden() throws {
        guard case .hidden = availability(.promptOption(9), capabilities: yes, item: try waitingItem(), option: nil)
        else { return XCTFail() }
    }

    func testKindMismatchIsHidden() throws {
        guard case .hidden = availability(.opApprove, capabilities: yes, item: try waitingItem(kind: "prompt"), option: nil)
        else { return XCTFail() }
        XCTAssertEqual(availability(.opApprove, capabilities: yes, item: try waitingItem(kind: "op"), option: nil), .available)
    }

    func testAgentAndRunVerbsNeedNoItem() {
        XCTAssertEqual(availability(.agentPause(reason: nil), capabilities: yes, item: nil, option: nil), .available)
        XCTAssertEqual(availability(.runResume(reason: nil), capabilities: yes, item: nil, option: nil), .available)
    }
}
