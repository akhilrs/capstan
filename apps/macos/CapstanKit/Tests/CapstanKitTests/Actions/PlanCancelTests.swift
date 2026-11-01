import XCTest
@testable import CapstanKit

/// `plan cancel` (section 11, Cancel a plan): a plan, a draft or one in progress, bound to the plan as it is listed.
final class PlanCancelTests: XCTestCase {
    func testArgumentsAndRow() throws {
        let request = makeRequest(verb: .planCancel, requestId: "plan:plan-3")
        XCTAssertEqual(
            try request.remoteArguments(),
            ["decide", "--project", "proj-1", "--json", "plan:plan-3", "--expect", testHash, "--nonce", testNonce,
             "--client", "Mac", "cancel"])
        XCTAssertEqual(DecideVerb.planCancel.row, DecideVerbRow(kind: "plan", verb: "cancel", outcome: "applied"))
        XCTAssertTrue(DecideVerb.planCancel.isDestructive, "a cancel confirms first")
        XCTAssertTrue(DecideVerbRow.documented.contains(DecideVerb.planCancel.row))
    }

    func testAPlanItemCarriesItsRequestAndAnOlderHostNone() throws {
        let base = "{\"key\":\"3\",\"planId\":\"plan-3\",\"sequence\":3,\"title\":\"Old draft\",\"state\":\"draft\","
            + "\"packages\":{\"total\":0,\"assigned\":0,\"reported\":0,\"integrated\":0}"
        let item = try PlanItem(json: try decodeJSON(Data(
            (base + ",\"requestId\":\"plan:plan-3\",\"contentHash\":\"\(testHash)\"}").utf8)))
        XCTAssertEqual(item.requestId, "plan:plan-3")
        XCTAssertEqual(item.contentHash, testHash)
        let stored = StoredPlan(item)
        XCTAssertEqual(try StoredPlan(PlanItem(json: stored.json)).requestId, "plan:plan-3", "kept through the store")
        let older = try PlanItem(json: try decodeJSON(Data((base + "}").utf8)))
        XCTAssertNil(older.requestId)
        XCTAssertNil(older.contentHash)
    }
}
