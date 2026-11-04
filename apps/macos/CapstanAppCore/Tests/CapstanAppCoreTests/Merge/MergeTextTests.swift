import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class MergeTextTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_700_000_000)

    private func text(_ raw: String) -> UntrustedText { UntrustedText(raw: raw, verified: true, truncated: false) }

    private func refusal(_ wire: String, message: String = "") -> DecideOutcome {
        .refused(DecideRefusal(code: DecideErrorCode(wire: wire), message: text(message)), hint: .refresh)
    }

    private func make(_ outcome: DecideOutcome) -> MergeText {
        MergeText.make(outcome, branch: "int/a", hostName: "build-1", at: now)
    }

    func testEveryRefusalMapsToItsFix() {
        let fixes: [(String, MergeText.Fix)] = [
            ("merge_conflict", .askPM), ("merge_disabled", .askPM), ("run_paused", .resumeProject), ("busy", .none),
            ("not_reviewed", .askPM), ("not_verified", .askPM), ("branch_moved", .askPM), ("wrong_branch", .askPM),
            ("dirty_tree", .askPM), ("unsafe_config", .askPM), ("no_identity", .askPM), ("git_too_old", .askPM),
            ("invalid_commit", .askPM), ("invalid_branch", .askPM), ("git_unavailable", .askPM),
            ("merge_failed", .askPM), ("something_else", .askPM),
            ("merge_unrecorded", .none), ("not_waiting", .none), ("hash_mismatch", .none),
        ]
        for (wire, fix) in fixes {
            XCTAssertEqual(make(refusal(wire)).fix, fix, wire)
        }
    }

    func testTonesAndTitlesOfRefusals() {
        XCTAssertEqual(make(refusal("merge_conflict")).tone, .bad)
        XCTAssertEqual(make(refusal("merge_conflict")).title, "The merge has conflicts")
        XCTAssertEqual(make(refusal("run_paused")).tone, .warning)
        XCTAssertEqual(make(refusal("run_paused")).title, "Refused: the project is paused")
        XCTAssertEqual(make(refusal("run_paused")).line, "Resume the project first. Nothing was merged.")
        XCTAssertEqual(make(refusal("not_waiting")).title, "No longer waiting")
        XCTAssertEqual(make(refusal("hash_mismatch")).headline, "Nothing was merged")
        XCTAssertEqual(make(refusal("whatever")).reason, "the host refused it (whatever)")
        XCTAssertTrue(make(refusal("busy")).line.contains("Try again when it is done."))
        XCTAssertTrue(make(refusal("git_too_old")).line.contains("build-1"))
    }

    func testHostTextIsKeptOnlyWhereItHelps() {
        XCTAssertEqual(make(refusal("merge_conflict", message: "a.txt")).hostText?.raw, "a.txt")
        XCTAssertEqual(make(refusal("dirty_tree", message: "b.txt")).hostText?.raw, "b.txt")
        XCTAssertNil(make(refusal("busy", message: "ignored")).hostText)
        XCTAssertNil(make(refusal("merge_conflict")).hostText, "an empty message is no text")
    }

    func testNotSentAndMalformed() {
        let timeout = make(.notSent(reason: .timeout, pendingId: UUID()))
        XCTAssertEqual(timeout.tone, .warning)
        XCTAssertEqual(timeout.title, "No answer yet")
        let down = make(.notSent(reason: .couldNotStart, pendingId: UUID()))
        XCTAssertEqual(down.tone, .bad)
        XCTAssertEqual(down.title, "Could not reach build-1")
        let odd = make(.malformed("x", pendingId: nil))
        XCTAssertEqual(odd.title, "Unexpected answer")
        XCTAssertEqual(odd.fix, .none)
    }

    func testMergedOutcomes() {
        func result(_ merge: MergeResult?) -> DecideResult {
            DecideResult(
                decisionId: "d", requestId: "r", nonce: "n", outcome: .applied, isRepeat: false, kind: "integration",
                verb: "merge", contentHash: "h", merge: merge)
        }
        XCTAssertEqual(make(.applied(result(nil))).title, "Merged")
        let merged = MergeResult(
            integrationId: "i", branch: "int/a", mergeCommit: "0123456789abcdef", alreadyInHead: false,
            verify: .notConfigured, cleanupScheduled: true)
        let done = make(.applied(result(merged)))
        XCTAssertEqual(done.title, "Merged, not pushed")
        XCTAssertEqual(done.headline, "int/a is merged")
        XCTAssertTrue(done.line.contains(" as 0123456789ab "))
        XCTAssertTrue(done.line.hasSuffix("The integration branch is cleaned up."))
        let already = MergeResult(
            integrationId: "i", branch: "int/a", mergeCommit: nil, alreadyInHead: true, verify: .notConfigured,
            cleanupScheduled: false)
        XCTAssertEqual(make(.forwarded(result(already))).title, "Already in the branch, confirmed")
    }
}
