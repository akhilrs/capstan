import XCTest
@testable import CapstanKit

final class PmMessageTests: XCTestCase {
    func testArgumentsAndRow() throws {
        let request = makeRequest(verb: .pmMessage("Which database for the tests?"), requestId: "pm")
        XCTAssertEqual(
            try request.remoteArguments(),
            ["decide", "--project", "proj-1", "--json", "pm", "--expect", testHash, "--nonce", testNonce, "--client", "Mac",
             "message", "Which database for the tests?"])
        XCTAssertEqual(DecideVerb.pmMessage("x").row, DecideVerbRow(kind: "pm", verb: "message", outcome: "applied"))
        XCTAssertTrue(DecideVerbRow.documented.contains(DecideVerb.pmMessage("x").row))
        XCTAssertFalse(DecideVerb.pmMessage("x").isDestructive)
    }

    func testTheTextIsCheckedLikeEveryText() {
        XCTAssertThrowsError(try makeRequest(verb: .pmMessage("   "), requestId: "pm").remoteArguments())
        XCTAssertThrowsError(try makeRequest(verb: .pmMessage("-rf"), requestId: "pm").remoteArguments())
        XCTAssertThrowsError(
            try makeRequest(verb: .pmMessage(String(repeating: "x", count: ProtocolLimits.reasonCap + 1)), requestId: "pm")
                .remoteArguments())
    }

    func testTheBindingIsTheHashOfTheActivePm() {
        // The host's value: sha256 of the canonical JSON {"pm":"pm-1"}.
        XCTAssertEqual(PmBinding.hash(agentId: "pm-1"), "a0a41d5aa96fcd0400fd3488d31030040a3e4b8eafbe0801e70e47c85dd76ca3")
        XCTAssertEqual(PmBinding.jsonString("a\"b"), "\"a\\\"b\"")
    }

    func testAvailableWithTheDecideCapability() {
        let yes = HostCapabilities(reachable: true, events: true, decide: true)
        let no = HostCapabilities(reachable: true, events: true, decide: false)
        XCTAssertEqual(availability(.pmMessage("x"), capabilities: yes, item: nil, option: nil), .available)
        if case .available = availability(.pmMessage("x"), capabilities: no, item: nil, option: nil) { XCTFail() }
    }
}
