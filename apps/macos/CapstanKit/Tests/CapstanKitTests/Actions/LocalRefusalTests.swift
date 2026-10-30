import XCTest
@testable import CapstanKit

final class LocalRefusalTests: XCTestCase {
    private func expectError(_ request: DecideRequest, _ error: DecideRequestError, line: UInt = #line) {
        XCTAssertThrowsError(try request.remoteArguments(), line: line) {
            XCTAssertEqual($0 as? DecideRequestError, error, line: line)
        }
    }

    func testExpect() {
        expectError(makeRequest(verb: .opApprove, expect: "0123456789a"), .expect)
        expectError(makeRequest(verb: .opApprove, expect: "0123456789AB"), .expect)
        expectError(makeRequest(verb: .opApprove, expect: String(repeating: "a", count: 65)), .expect)
        expectError(makeRequest(verb: .opApprove, expect: "0123456789ag"), .expect)
        XCTAssertNoThrow(try makeRequest(verb: .opApprove, expect: String(repeating: "a", count: 64)).remoteArguments())
    }

    func testClient() {
        expectError(makeRequest(verb: .opApprove, client: "Mac\u{7}"), .client)
        expectError(makeRequest(verb: .opApprove, client: ""), .client)
        expectError(makeRequest(verb: .opApprove, client: String(repeating: "a", count: 65)), .client)
        expectError(makeRequest(verb: .opApprove, client: "a/b"), .client)
        XCTAssertNoThrow(try makeRequest(verb: .opApprove, client: "My Mac.2_x-y").remoteArguments())
    }

    func testNonce() {
        expectError(makeRequest(verb: .opApprove, nonce: "short"), .nonce)
        expectError(makeRequest(verb: .opApprove, nonce: String(repeating: "a", count: 21) + "$"), .nonce)
    }

    func testFreeText() {
        expectError(makeRequest(verb: .messageReply("")), .text("reply"))
        expectError(makeRequest(verb: .messageReply("   ")), .text("reply"))
        expectError(makeRequest(verb: .opDeny(note: String(repeating: "a", count: 1025))), .text("note"))
        expectError(makeRequest(verb: .promptOption(-1)), .optionNumber)
        expectError(makeRequest(verb: .promptOption(0)), .optionNumber)
    }

    func testTextAtTheCapIsNotCut() throws {
        let exact = String(repeating: "a", count: 1024)
        let words = try makeRequest(verb: .opDeny(note: exact)).remoteArguments()
        XCTAssertEqual(words.last, exact)
        // 512 two-byte characters are exactly 1024 bytes and stay whole; one more is refused, not cut.
        let wide = String(repeating: "é", count: 512)
        XCTAssertEqual(try makeRequest(verb: .opDeny(note: wide)).remoteArguments().last, wide)
        expectError(makeRequest(verb: .opDeny(note: wide + "é")), .text("note"))
        // A multibyte character that straddles the cap is refused whole.
        expectError(makeRequest(verb: .opDeny(note: String(repeating: "a", count: 1023) + "é")), .text("note"))
    }

    func testControlAndBidiBecomeSpaces() throws {
        let words = try makeRequest(verb: .messageReply("a\u{0}b\u{202E}c\u{2066}d\te\u{85}f")).remoteArguments()
        XCTAssertEqual(words.last, "a b c d e f")
    }

    func testNoncesDifferAndUseTheCharset() {
        let allowed = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-")
        let a = Nonce.make()
        let b = Nonce.make()
        XCTAssertNotEqual(a, b)
        for nonce in [a, b] {
            XCTAssertEqual(nonce.count, 22)
            XCTAssertTrue(nonce.allSatisfy { allowed.contains($0) })
        }
        XCTAssertNotEqual(
            DecideRequest(hostId: UUID(), projectId: "p", requestId: "op:1", expect: testHash, verb: .opApprove).nonce,
            DecideRequest(hostId: UUID(), projectId: "p", requestId: "op:1", expect: testHash, verb: .opApprove).nonce)
    }

    func testSendRefusesLocallyAndKeepsNothing() async {
        let runner = FakeRunner()
        let store = PendingStore(url: tempStoreURL())
        let decider = Decider(runner: runner, pending: store, clock: TestClock())
        let outcome = await decider.send(makeRequest(verb: .messageReply("")), config: makeConfig())
        guard case .refused(let refusal, let hint) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(refusal.code, .invalidRequest)
        XCTAssertEqual(hint, .invalid)
        XCTAssertEqual(runner.startCount, 0)
        let kept = await store.all()
        XCTAssertTrue(kept.isEmpty)
    }
}
