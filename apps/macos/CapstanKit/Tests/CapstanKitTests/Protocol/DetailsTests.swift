import XCTest
@testable import CapstanKit

final class DetailsTests: XCTestCase {
    private func plan(_ extra: String) throws -> PlanItem {
        let json = "{\"key\":\"plan:1\",\"planId\":\"p\",\"sequence\":1,\"title\":\"T\",\"state\":\"approved\","
            + "\"packages\":{\"total\":2,\"assigned\":1,\"reported\":0,\"integrated\":0}\(extra)}"
        return try PlanItem(json: try decodeJSON(Data(json.utf8)))
    }

    private func waiting(_ extra: String) throws -> WaitingItem {
        let json = "{\"key\":\"integration:1\",\"requestId\":\"integration:i\",\"kind\":\"integration\",\"since\":\"s\","
            + "\"summary\":\"m\",\"contentHash\":\"h\",\"needsTerminal\":false\(extra)}"
        return try WaitingItem(json: try decodeJSON(Data(json.utf8)))
    }

    func testPlanWithDetails() throws {
        let item = try plan(
            ",\"packageDetails\":[{\"id\":\"a\",\"title\":\"A\",\"role\":\"developer\",\"assigneeAgentId\":\"dev-1\","
                + "\"state\":\"reported\",\"dependsOn\":[\"b\"],\"review\":\"findings\",\"integrationId\":\"i1\","
                + "\"integrationState\":\"conflicted\"},{\"id\":\"b\",\"title\":\"B\",\"role\":\"\",\"assigneeAgentId\":null,"
                + "\"state\":\"unassigned\",\"dependsOn\":[],\"review\":null,\"integrationId\":null,"
                + "\"integrationState\":null}],\"packageDetailsOmitted\":5")
        XCTAssertEqual(item.packageDetails?.count, 2)
        XCTAssertEqual(item.packageDetailsOmitted, 5)
        let first = try XCTUnwrap(item.packageDetails?.first)
        XCTAssertEqual(first.state, .reported)
        XCTAssertEqual(first.dependsOn, ["b"])
        XCTAssertEqual(first.review, "findings")
        XCTAssertEqual(first.assigneeAgentId, "dev-1")
        XCTAssertEqual(first.integrationState, "conflicted")
        XCTAssertNil(item.packageDetails?.last?.review)
    }

    func testPlanWithoutDetailsIsNil() throws {
        let item = try plan("")
        XCTAssertNil(item.packageDetails)
        XCTAssertNil(item.packageDetailsOmitted)
    }

    func testUnknownStateIsOther() throws {
        let item = try plan(
            ",\"packageDetails\":[{\"id\":\"a\",\"title\":\"A\",\"role\":\"r\",\"state\":\"paused\",\"dependsOn\":[]}]")
        XCTAssertEqual(item.packageDetails?.first?.state, .other("paused"))
    }

    func testWaitingIntegrationFields() throws {
        let item = try waiting(",\"integrationState\":\"merged\",\"branch\":\"feat/x\",\"headSha\":\"abc1234\"")
        XCTAssertEqual(item.integrationState, "merged")
        XCTAssertEqual(item.branch?.raw, "feat/x")
        XCTAssertEqual(item.headSha, "abc1234")
    }

    func testBadHeadShaBecomesNil() throws {
        for bad in ["abc12", "ABC1234", "zzzzzzz", String(repeating: "a", count: 65)] {
            XCTAssertNil(try waiting(",\"headSha\":\"\(bad)\"").headSha, bad)
        }
        XCTAssertNil(try waiting(",\"headSha\":null,\"branch\":null").branch)
        XCTAssertNil(try waiting("").integrationState)
    }

    func testStoredPlanFromBeforeDecodes() throws {
        let old = "{\"key\":\"plan:1\",\"planId\":\"p\",\"sequence\":1,\"title\":{\"raw\":\"T\",\"verified\":true,"
            + "\"truncated\":false},\"state\":\"approved\",\"packages\":{\"total\":1,\"assigned\":0,\"reported\":0,"
            + "\"integrated\":0}}"
        let stored = try JSONDecoder().decode(StoredPlan.self, from: Data(old.utf8))
        XCTAssertNil(stored.packageDetails)
        let oldWaiting = "{\"key\":\"k\",\"requestId\":\"r\",\"kind\":\"integration\",\"since\":\"s\",\"summary\":{\"raw\":\"m\","
            + "\"verified\":true,\"truncated\":false},\"contentHash\":\"h\",\"needsTerminal\":false}"
        XCTAssertNil(try JSONDecoder().decode(StoredWaiting.self, from: Data(oldWaiting.utf8)).headSha)
    }

    func testJsonRoundTripKeepsFields() throws {
        let item = try plan(
            ",\"packageDetails\":[{\"id\":\"a\",\"title\":\"A\",\"role\":\"r\",\"state\":\"cancelled\",\"dependsOn\":[\"b\"],"
                + "\"review\":\"passed\"}],\"packageDetailsOmitted\":1")
        let again = try PlanItem(json: StoredPlan(item).json)
        XCTAssertEqual(again.packageDetails, item.packageDetails)
        XCTAssertEqual(again.packageDetailsOmitted, 1)
        let wait = try waiting(",\"integrationState\":\"merged\",\"branch\":\"b\",\"headSha\":\"abc1234\"")
        let back = try WaitingItem(json: StoredWaiting(wait).json)
        XCTAssertEqual(back.headSha, "abc1234")
        XCTAssertEqual(back.branch, wait.branch)
        XCTAssertEqual(back.integrationState, "merged")
    }
}
