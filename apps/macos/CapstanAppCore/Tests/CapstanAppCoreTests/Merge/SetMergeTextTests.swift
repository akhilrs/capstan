import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class SetMergeTextTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_700_000_000)

    private func member(_ repo: String, _ position: Int64, _ state: String, dropped: Bool = false) -> SetMember {
        SetMember(
            repo: repo, position: position, integrationId: "i-\(repo)", state: state, branch: "b",
            headSha: "3f2a9c1d5b7e", dropped: dropped)
    }

    private func set(repo: String) -> IntegrationSet {
        let members = [member("api", 0, "confirmed"), member("web", 1, "merged"), member("worker", 2, "merged")]
        return IntegrationSet(
            setId: "8a46cad2", state: "merged", repo: repo, position: members.first { $0.repo == repo }?.position ?? 0,
            members: members)
    }

    private func text(_ raw: String) -> UntrustedText { UntrustedText(raw: raw, verified: true, truncated: false) }

    private func refused(_ wire: String, _ message: String) -> DecideOutcome {
        .refused(DecideRefusal(code: DecideErrorCode(wire: wire), message: text(message)), hint: .refresh)
    }

    func testTheSheetNamesTheRepoItsPlaceAndWhatFollows() {
        let sheet = SetMergeSheet.make(set(repo: "web"), verify: nil)
        XCTAssertEqual(sheet.title, "Merge web into main")
        XCTAssertEqual(sheet.order, "Repo 2 of 3 in merge order")
        XCTAssertEqual(sheet.after, "After this: worker")
        XCTAssertEqual(sheet.action, "Merge web")
        XCTAssertNil(sheet.blockedBy)
        XCTAssertEqual(sheet.members.map(\.repo), ["api", "web", "worker"])
    }

    func testTheLastRepoSaysSo() {
        let sheet = SetMergeSheet.make(set(repo: "worker"), verify: nil, mainBranch: "release")
        XCTAssertEqual(sheet.title, "Merge worker into release")
        XCTAssertEqual(sheet.after, "This is the last repo of the set.")
        XCTAssertEqual(sheet.blockedBy, "web")
        XCTAssertEqual(sheet.blockedTitle, "worker has to wait")
    }

    func testABlockedMemberIsNotOfferedMergeAndANotBuiltBlockerIsSaid() {
        let members = [member("api", 0, "conflicted"), member("web", 1, "merged")]
        let blocked = IntegrationSet(setId: "s", state: "conflicted", repo: "web", position: 1, members: members)
        let sheet = SetMergeSheet.make(blocked, verify: nil)
        XCTAssertEqual(sheet.blockedBy, "api")
        XCTAssertFalse(sheet.canMerge)
        XCTAssertEqual(sheet.blockedNote, "api has a conflict and has nothing to merge yet. It needs a redo or a fix first.")
        XCTAssertNil(sheet.ownNote)
    }

    func testAConfirmedMemberOffersNoMerge() {
        let done = IntegrationSet(
            setId: "s", state: "merged", repo: "api", position: 0, members: [member("api", 0, "confirmed"), member("web", 1, "merged")])
        let sheet = SetMergeSheet.make(done, verify: nil)
        XCTAssertFalse(sheet.canMerge)
        XCTAssertEqual(sheet.ownNote, "api is already in main.")
    }

    func testAMemberWithNothingToMergeOffersNoMerge() {
        for (state, word) in [("running", "is still running"), ("conflicted", "has a conflict"), ("failed", "failed")] {
            let own = IntegrationSet(
                setId: "s", state: state, repo: "api", position: 0, members: [member("api", 0, state), member("web", 1, "merged")])
            let sheet = SetMergeSheet.make(own, verify: nil)
            XCTAssertNil(sheet.blockedBy)
            XCTAssertFalse(sheet.canMerge)
            XCTAssertEqual(sheet.ownNote, "api " + word + " and has nothing to merge yet.")
        }
        XCTAssertTrue(SetMergeSheet.make(set(repo: "web"), verify: nil).canMerge)
    }

    func testAnOrderRefusalIsQuotedAsTheHostSentIt() {
        let made = MergeText.make(
            refused("not_waiting", "merge web first"), branch: "b", hostName: "h", at: now, set: set(repo: "worker"))
        XCTAssertEqual(made.title, "merge web first")
        XCTAssertEqual(made.headline, "worker has to wait")
        XCTAssertEqual(made.openRepo, "web")
        XCTAssertEqual(made.line, "The set merges one repo at a time, in merge order. Nothing was merged.")
        XCTAssertEqual(made.tone, .warning)
    }

    func testARefusalThatLooksLikeTheOrderOneButNamesNoMemberIsNotRead() {
        let made = MergeText.make(
            refused("not_waiting", "merge nowhere first"), branch: "b", hostName: "h", at: now, set: set(repo: "worker"))
        XCTAssertNil(made.openRepo)
        XCTAssertEqual(made.title, "No longer waiting")
        let unverified = DecideOutcome.refused(
            DecideRefusal(code: .notWaiting, message: UntrustedText(raw: "merge web first", verified: false, truncated: false)),
            hint: .refresh)
        XCTAssertNil(MergeText.make(unverified, branch: "b", hostName: "h", at: now, set: set(repo: "worker")).openRepo)
    }

    func testAMergeOfAMemberSaysItIsInMainAndNamesTheNext() {
        let merge = MergeResult(
            integrationId: "i-web", branch: "b", mergeCommit: "7d1c0e9aabbccddeeff0011223344556677889900",
            alreadyInHead: false, verify: .notConfigured, cleanupScheduled: false)
        let result = DecideResult(
            decisionId: "d", requestId: "r", nonce: "n", outcome: .applied, isRepeat: false, kind: "integration",
            verb: "merge", contentHash: "h", merge: merge)
        let made = MergeText.make(.applied(result), branch: "b", hostName: "h", at: now, set: set(repo: "web"))
        XCTAssertEqual(made.headline, "web is in main · 7d1c0e9")
        XCTAssertEqual(made.line, "web is confirmed. The PM was told.")
        XCTAssertEqual(made.nextRepo, "worker")
        XCTAssertEqual(made.tone, .good)
    }

    func testAMergeThatWasNotRecordedShowsTheConfirmCommand() {
        let made = MergeText.make(
            refused("merge_unrecorded", ""), branch: "b", hostName: "h", at: now, set: set(repo: "web"))
        XCTAssertEqual(made.headline, "Confirm web from the terminal")
        XCTAssertEqual(made.command, "cstan integrate confirm 8a46cad2 --repo web")
        XCTAssertEqual(
            made.line, "The merge commit is on main. Recording it failed, so worker waits until web is confirmed.")
    }

    func testASingleRepoMergeKeepsItsText() {
        let made = MergeText.make(refused("not_waiting", "merge web first"), branch: "b", hostName: "h", at: now)
        XCTAssertEqual(made.title, "No longer waiting")
        XCTAssertNil(made.openRepo)
        XCTAssertNil(made.command)
    }
}
