import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class PackageRowTests: XCTestCase {
    private func text(_ raw: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: false)
    }

    private func detail(
        _ id: String, state: PackageState, title: UntrustedText? = nil, review: String? = nil,
        dependsOn: [String] = [], assignee: String? = nil, integration: String? = nil
    ) -> PackageDetail {
        PackageDetail(
            id: id, title: title ?? text("Title \(id)"), role: "developer", assigneeAgentId: assignee, state: state,
            dependsOn: dependsOn, review: review, integrationId: nil, integrationState: integration)
    }

    private func plan(_ details: [PackageDetail]?, omitted: Int64? = nil) -> StoredPlan {
        StoredPlan(
            key: "plan:p", planId: "p", sequence: 1, title: text("Plan"), state: "approved",
            packages: StoredCounts(total: 4, assigned: 0, reported: 0, integrated: 0), packageDetails: details,
            packageDetailsOmitted: omitted)
    }

    func testOrderIsTheHostsAndCancelledStayLast() {
        let rows = PackageRow.rows(
            plan([detail("b", state: .assigned), detail("a", state: .unassigned), detail("z", state: .cancelled)]))
        XCTAssertEqual(rows.map(\.id), ["b", "a", "z"])
        XCTAssertEqual(rows.last?.stateWord, "Cancelled")
    }

    func testStateAndReviewWords() {
        let rows = PackageRow.rows(
            plan([
                detail("a", state: .reported, review: "passed", dependsOn: ["x"], assignee: "dev-1"),
                detail("b", state: .integrated, review: "findings", integration: "merged"),
                detail("c", state: .other("later")),
            ]))
        XCTAssertEqual(rows[0].stateWord, "Reported")
        XCTAssertEqual(rows[0].reviewWord, "Passed")
        XCTAssertEqual(rows[0].dependsOn, ["x"])
        XCTAssertEqual(rows[0].assignee, "dev-1")
        XCTAssertNil(rows[0].integrationWord)
        XCTAssertEqual(rows[1].reviewWord, "Changes requested")
        XCTAssertEqual(rows[1].integrationWord, "Merged")
        XCTAssertEqual(rows[2].reviewWord, "Not reviewed")
        XCTAssertEqual(rows[2].stateWord, "later")
    }

    func testUnverifiedTitleShowsThePackageId() {
        let rows = PackageRow.rows(plan([detail("pkg-1", state: .assigned, title: text("secret", verified: false))]))
        XCTAssertEqual(rows[0].title, "pkg-1")
    }

    func testOmittedCountAndMissingDetails() {
        XCTAssertEqual(PlanRow(plan([], omitted: 3)).omittedPackages, 3)
        XCTAssertEqual(PlanRow(plan(nil)).omittedPackages, 0)
        XCTAssertEqual(PackageRow.rows(plan(nil)), [])
    }
}
