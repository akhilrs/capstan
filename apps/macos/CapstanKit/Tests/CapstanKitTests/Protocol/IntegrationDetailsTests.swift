import XCTest
@testable import CapstanKit

final class IntegrationDetailsTests: XCTestCase {
    private func text(_ key: String, _ value: String, verified: Bool = true, truncated: Bool = false) -> String {
        "\"\(key)\":\"\(value)\",\"textVerified\":\(verified),\"truncated\":\(truncated)"
    }

    private var fullDetails: String {
        let plan = "{\"planId\":\"plan-3\"," + text("title", "Remote protocol") + "}"
        let report = "{\"reportId\":\"rep-9\",\"packageId\":null," + text("summary", "Added the feed") + "}"
        let signoff = "{\"planId\":\"plan-4\",\"createdAt\":\"2026-10-01T10:00:00.000Z\"," + text("summary", "Signed off") + "}"
        let area = "{" + text("path", "rust/crates/kernel") + ",\"files\":9}"
        let migration = "{" + text("path", "migrations/0053.sql") + "}"
        let step = "{" + text("text", "Run the migrations") + "}"
        let impact = "{\"filesChanged\":12,\"insertions\":340,\"deletions\":25,\"truncated\":true,"
            + "\"areas\":[\(area)],\"areasOmitted\":2,\"migrations\":[\(migration)],\"migrationsOmitted\":1,"
            + "\"nextSteps\":[\(step)]}"
        let review = "{\"verdict\":\"findings\",\"reviewerAgentId\":\"reviewer-2\",\"round\":2}"
        let verify = "{\"latest\":{\"state\":\"failed\",\"commitSha\":\"3f2a9c1d5b7e\",\"finishedAt\":\"2026-10-01T09:50:00.000Z\"},"
            + "\"lastPassed\":{\"commitSha\":\"1111111aaaa\",\"finishedAt\":\"2026-09-30T09:50:00.000Z\"}}"
        return "{\"recorded\":true,\"impactError\":null,\"plans\":[\(plan)],\"plansOmitted\":1,"
            + "\"reports\":[\(report)],\"reportsOmitted\":3,\"signoffs\":[\(signoff)],\"signoffsOmitted\":0,"
            + "\"impact\":\(impact),\"review\":\(review),\"verify\":\(verify)}"
    }

    private func waitingJSON(_ extra: String) -> String {
        "{\"key\":\"integration:1\",\"requestId\":\"integration:i\",\"kind\":\"integration\",\"since\":\"s\","
            + "\"summary\":\"merged b\",\"contentHash\":\"h\",\"needsTerminal\":false,"
            + "\"integrationState\":\"merged\",\"branch\":\"b\",\"headSha\":\"3f2a9c1d5b7e\"\(extra)}"
    }

    private func waiting(_ extra: String) throws -> WaitingItem {
        try WaitingItem(json: try decodeJSON(Data(waitingJSON(extra).utf8)))
    }

    func testAFullObjectParses() throws {
        let item = try waiting(",\"integrationDetails\":\(fullDetails)")
        let details = try XCTUnwrap(item.integrationDetails)
        XCTAssertTrue(details.recorded)
        XCTAssertNil(details.impactError)
        XCTAssertEqual(details.plans.first?.planId, "plan-3")
        XCTAssertEqual(details.plans.first?.title, UntrustedText(raw: "Remote protocol", verified: true, truncated: false))
        XCTAssertEqual(details.plansOmitted, 1)
        XCTAssertEqual(details.reports.first?.reportId, "rep-9")
        XCTAssertNil(details.reports.first?.packageId)
        XCTAssertEqual(details.reportsOmitted, 3)
        XCTAssertEqual(details.signoffs.first?.createdAt, "2026-10-01T10:00:00.000Z")
        let impact = try XCTUnwrap(details.impact)
        XCTAssertEqual(impact.filesChanged, 12)
        XCTAssertEqual(impact.insertions, 340)
        XCTAssertEqual(impact.deletions, 25)
        XCTAssertTrue(impact.truncated)
        XCTAssertEqual(impact.areas.first?.files, 9)
        XCTAssertEqual(impact.areasOmitted, 2)
        XCTAssertEqual(impact.migrations.first?.path.raw, "migrations/0053.sql")
        XCTAssertEqual(impact.migrationsOmitted, 1)
        XCTAssertEqual(impact.nextSteps.first?.text.raw, "Run the migrations")
        XCTAssertEqual(details.review, IntegrationReview(verdict: "findings", reviewerAgentId: "reviewer-2", round: 2))
        XCTAssertEqual(details.verify?.latest.state, "failed")
        XCTAssertEqual(details.verify?.latest.commitSha, "3f2a9c1d5b7e")
        XCTAssertEqual(details.verify?.lastPassed?.commitSha, "1111111aaaa")
    }

    func testRecordedFalseParses() throws {
        let item = try waiting(",\"integrationDetails\":{\"recorded\":false}")
        XCTAssertEqual(item.integrationDetails, IntegrationDetails(recorded: false))
    }

    func testAbsentOrNullGivesNil() throws {
        XCTAssertNil(try waiting("").integrationDetails)
        XCTAssertNil(try waiting(",\"integrationDetails\":null").integrationDetails)
    }

    func testABadHeadShaDoesNotCostTheDetails() throws {
        let json = waitingJSON(",\"integrationDetails\":\(fullDetails)").replacingOccurrences(
            of: "\"headSha\":\"3f2a9c1d5b7e\"", with: "\"headSha\":\"ABC12\"")
        let item = try WaitingItem(json: try decodeJSON(Data(json.utf8)))
        XCTAssertNil(item.headSha)
        XCTAssertEqual(item.integrationDetails?.recorded, true)
    }

    func testWrongTypesAreDropped() throws {
        XCTAssertNil(try waiting(",\"integrationDetails\":\"text\"").integrationDetails)
        XCTAssertNil(try waiting(",\"integrationDetails\":{\"recorded\":\"yes\"}").integrationDetails)
        let mixed = "{\"recorded\":true,\"plans\":\"none\",\"plansOmitted\":\"x\",\"reports\":[7,{\"reportId\":\"r\"}],"
            + "\"impact\":{\"filesChanged\":\"12\",\"insertions\":1,\"deletions\":1},\"review\":[],\"verify\":{\"latest\":1},"
            + "\"impactError\":4}"
        let details = try XCTUnwrap(try waiting(",\"integrationDetails\":\(mixed)").integrationDetails)
        XCTAssertEqual(details.plans, [])
        XCTAssertEqual(details.plansOmitted, 0)
        XCTAssertEqual(details.reports, [])
        XCTAssertNil(details.impact)
        XCTAssertNil(details.review)
        XCTAssertNil(details.verify)
        XCTAssertNil(details.impactError)
    }

    func testABadVerifyShaDropsTheVerify() throws {
        let bad = fullDetails.replacingOccurrences(of: "\"commitSha\":\"3f2a9c1d5b7e\"", with: "\"commitSha\":\"XYZ\"")
        XCTAssertNil(try waiting(",\"integrationDetails\":\(bad)").integrationDetails?.verify)
    }

    func testTextFlagsAreKeptAndAMissingFlagIsNotVouchedFor() throws {
        let plan = "{\"planId\":\"p\"," + text("title", "T", verified: false, truncated: true) + "}"
        let plain = "{\"reportId\":\"r\",\"summary\":\"no flags\"}"
        let body = "{\"recorded\":true,\"plans\":[\(plan)],\"reports\":[\(plain)]}"
        let details = try XCTUnwrap(try waiting(",\"integrationDetails\":\(body)").integrationDetails)
        XCTAssertFalse(try XCTUnwrap(details.plans.first).title.verified)
        XCTAssertEqual(details.plans.first?.title.raw, "")
        XCTAssertTrue(try XCTUnwrap(details.plans.first).title.truncated)
        XCTAssertFalse(try XCTUnwrap(details.reports.first).summary.verified)
    }

    func testAnOldStoredWaitingDecodes() throws {
        let old = "{\"key\":\"k\",\"requestId\":\"r\",\"kind\":\"integration\",\"since\":\"s\",\"summary\":{\"raw\":\"m\","
            + "\"verified\":true,\"truncated\":false},\"contentHash\":\"h\",\"needsTerminal\":false,"
            + "\"integrationState\":\"merged\"}"
        let stored = try JSONDecoder().decode(StoredWaiting.self, from: Data(old.utf8))
        XCTAssertNil(stored.integrationDetails)
        XCTAssertEqual(stored.integrationState, "merged")
    }

    func testAStoredWaitingWithDetailsRoundTrips() throws {
        let item = try waiting(",\"integrationDetails\":\(fullDetails)")
        let stored = StoredWaiting(item)
        let again = try JSONDecoder().decode(StoredWaiting.self, from: try JSONEncoder().encode(stored))
        XCTAssertEqual(again, stored)
        XCTAssertEqual(again.integrationDetails, item.integrationDetails)
    }

    func testTheWireFormKeepsDetailsAndTheirFlags() throws {
        let plan = "{\"planId\":\"p\"," + text("title", "T", verified: false, truncated: true) + "}"
        let body = "{\"recorded\":true,\"plans\":[\(plan)],\"plansOmitted\":2}"
        let withFlags = try waiting(",\"integrationDetails\":\(body)")
        let back = try WaitingItem(json: StoredWaiting(withFlags).json)
        XCTAssertEqual(back.integrationDetails, withFlags.integrationDetails)
        let full = try waiting(",\"integrationDetails\":\(fullDetails)")
        XCTAssertEqual(try WaitingItem(json: StoredWaiting(full).json).integrationDetails, full.integrationDetails)
        let unrecorded = try waiting(",\"integrationDetails\":{\"recorded\":false}")
        XCTAssertEqual(
            try WaitingItem(json: StoredWaiting(unrecorded).json).integrationDetails, IntegrationDetails(recorded: false))
    }

    // MARK: reducer

    private func snapshotHarness(_ details: String) throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:1", item: waitingJSON(",\"integrationDetails\":\(details)")),
            try StoreWire.end("proj-a"))
        return harness
    }

    func testAReReadItemWithTheSameHashButNewDetailsReplacesTheStoredDetails() throws {
        var harness = try snapshotHarness("{\"recorded\":false}")
        XCTAssertEqual(harness.alpha?.waiting["integration:1"]?.integrationDetails, IntegrationDetails(recorded: false))
        let again = waitingJSON(",\"integrationDetails\":\(fullDetails)")
        harness.apply(
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:1", item: again),
            try StoreWire.end("proj-a"))
        let stored = try XCTUnwrap(harness.alpha?.waiting["integration:1"])
        XCTAssertEqual(stored.contentHash, "h")
        XCTAssertEqual(stored.integrationDetails?.recorded, true)
        XCTAssertEqual(stored.integrationDetails?.review?.verdict, "findings")
    }

    func testAnUpsertThatStatesDetailsReplacesThemAndOneThatDoesNotKeepsThem() throws {
        var harness = try snapshotHarness(fullDetails)
        let keep = try StoreWire.event(
            "review.verdict", effects: [("waiting", "upsert", "integration:1", "{\"summary\":\"merged b2\"}")])
        harness.apply(.event(projectId: "proj-a", keep))
        XCTAssertEqual(harness.alpha?.waiting["integration:1"]?.integrationDetails?.review?.round, 2)
        let replace = try StoreWire.event(
            "review.verdict", seq: 31,
            effects: [("waiting", "upsert", "integration:1", "{\"integrationDetails\":{\"recorded\":false}}")])
        harness.apply(.event(projectId: "proj-a", replace))
        XCTAssertEqual(harness.alpha?.waiting["integration:1"]?.integrationDetails, IntegrationDetails(recorded: false))
    }
}
