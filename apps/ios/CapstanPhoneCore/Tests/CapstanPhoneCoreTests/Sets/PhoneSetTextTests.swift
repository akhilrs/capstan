import CapstanAppCore
import CapstanKit
import XCTest
@testable import CapstanPhoneCore

final class PhoneSetTextTests: XCTestCase {
    private func member(_ repo: String, _ position: Int64, _ state: String, dropped: Bool = false) -> SetMember {
        SetMember(
            repo: repo, position: position, state: state, headSha: state == "running" ? nil : "9f3c2a1b0000",
            dropped: dropped)
    }

    private func set(_ repo: String, _ members: [SetMember]) -> IntegrationSet {
        IntegrationSet(
            setId: "8a46cad2", state: "merged", repo: repo, position: members.first { $0.repo == repo }!.position,
            reviewSubject: false, members: members)
    }

    private var three: [SetMember] { [member("api", 0, "confirmed"), member("web", 1, "merged"), member("worker", 2, "merged")] }

    func testRowStripAndReason() {
        let s = set("web", three)
        XCTAssertEqual(PhoneSetText.secondary(s), "●○○  Merge web next")
        XCTAssertEqual(SetText.title(s), "Set 8a46cad2 · 1 of 3 in main")
    }

    func testStripCapsAtEightWithPlusN() {
        let many = (0..<32).map { member("repo\($0)", Int64($0), "merged") }
        let s = set("repo0", many)
        let strip = SetText.strip(s, cap: PhoneSetText.stripCap)
        XCTAssertEqual(strip.members.count, 8)
        XCTAssertEqual(strip.trailing, 24)
        XCTAssertTrue(PhoneSetText.secondary(s).contains("+24"))
        XCTAssertTrue(PhoneSetText.stripVoiceover(s).hasSuffix("and 24 more repos"))
    }

    func testDroppedMemberIsOutsideTheCounts() {
        let s = set("web", three + [member("infra", 3, "discarded", dropped: true)])
        XCTAssertEqual(SetText.title(s), "Set 8a46cad2 · 1 of 3 in main")
        XCTAssertEqual(PhoneSetText.secondary(s), "●○○–  Merge web next")
        let rows = SetMemberRow.rows(s)
        XCTAssertEqual(rows.last?.sub, "Outside the gate, review and confirm")
        XCTAssertEqual(rows.last?.struck, true)
        XCTAssertFalse(rows.last?.isNext ?? true)
    }

    func testMergeSheetOrderAndNext() {
        let sheet = SetMergeSheet.make(set("web", three), verify: nil)
        XCTAssertEqual(sheet.title, "Merge web into main")
        XCTAssertEqual(sheet.order, "Repo 2 of 3 in merge order")
        XCTAssertEqual(sheet.after, "After this: worker")
        XCTAssertEqual(PhoneSetMergeOffer.make(sheet) { _ in nil }, .merge)
    }

    func testOnlyTheNextBuiltMemberOffersMerge() {
        // worker is built but web is next: merge web first, and Open only when web has a waiting item.
        let sheet = SetMergeSheet.make(set("worker", three), verify: nil)
        XCTAssertEqual(sheet.blockedBy, "web")
        XCTAssertEqual(sheet.blockedTitle, "worker has to wait")
        XCTAssertEqual(PhoneSetMergeOffer.make(sheet) { $0 == "web" ? "k1" : nil }, .blocked(open: "web"))
        XCTAssertEqual(PhoneSetMergeOffer.make(sheet) { _ in nil }, .blocked(open: nil))
    }

    func testNotBuiltBlockerSaysRedoOrFix() {
        var members = three
        members[1] = member("web", 1, "conflicted")
        let sheet = SetMergeSheet.make(set("worker", members), verify: nil)
        XCTAssertEqual(sheet.blockedNote, "web has a conflict and has nothing to merge yet. It needs a redo or a fix first.")
    }

    func testConfirmedAndDiscardedMembersOfferNoMerge() {
        for state in ["confirmed", "discarded"] {
            var members = three
            members[1] = member("web", 1, state)
            let sheet = SetMergeSheet.make(set("web", members), verify: nil)
            XCTAssertEqual(PhoneSetMergeOffer.make(sheet) { _ in nil }, .nothingToMerge, state)
        }
    }

    func testWaitingKeyFindsAnotherMemberOfTheSameSet() {
        let s = set("web", three)
        func item(_ key: String, _ repo: String, setId: String = "8a46cad2") -> StoredWaiting {
            var made = StoredWaiting(
                key: key, requestId: key, kind: .integration, since: "",
                summary: UntrustedText(raw: "", verified: true, truncated: false), contentHash: key, needsTerminal: false)
            made.set = IntegrationSet(
                setId: setId, state: "merged", repo: repo, position: 0, reviewSubject: false, members: three)
            return made
        }
        let waiting = [item("a", "api", setId: "other"), item("w", "worker")]
        XCTAssertEqual(PhoneSetText.waitingKey(of: "worker", in: s, among: waiting), "w")
        XCTAssertNil(PhoneSetText.waitingKey(of: "api", in: s, among: waiting))
    }
}
