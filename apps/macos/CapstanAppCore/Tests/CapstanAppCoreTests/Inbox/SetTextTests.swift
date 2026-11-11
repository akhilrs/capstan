import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class SetTextTests: XCTestCase {
    private func member(
        _ repo: String, _ position: Int64, _ state: String, attempt: Int64 = 1, dropped: Bool = false,
        head: String? = "3f2a9c1d5b7e"
    ) -> SetMember {
        SetMember(
            repo: repo, position: position, attempt: attempt, integrationId: "i-\(repo)", state: state, branch: "b",
            headSha: head, dropped: dropped)
    }

    private func set(
        _ members: [SetMember], repo: String = "web", state: String = "merged", subject: Bool = false
    ) -> IntegrationSet {
        IntegrationSet(
            setId: "8a46cad2", state: state, repo: repo, position: members.first { $0.repo == repo }?.position ?? 0,
            reviewSubject: subject, members: members)
    }

    private var partlyInMain: IntegrationSet {
        set([member("api", 0, "confirmed"), member("web", 1, "merged"), member("worker", 2, "merged")])
    }

    // MARK: vocabulary

    func testTheFeedsMergedReadsBuiltNeverMerged() {
        XCTAssertEqual(SetState(wire: "merged").word, "built")
        XCTAssertEqual(SetState(wire: "merged").longWord, "Built, not in main")
        XCTAssertEqual(SetState(wire: "confirmed").word, "in main")
        XCTAssertEqual(SetState(wire: "running").word, "building")
        XCTAssertEqual(SetState(wire: "something new").word, "building")
        let words = ["running", "merged", "confirmed", "conflicted", "failed", "discarded"].map { SetState(wire: $0).word }
        XCTAssertFalse(words.contains("merged"))
    }

    func testEveryStateHasItsOwnGlyph() {
        let glyphs = ["running", "merged", "confirmed", "conflicted", "failed", "discarded"].map { SetState(wire: $0).glyph }
        XCTAssertEqual(glyphs, ["◌", "○", "●", "▲", "✕", "–"])
        XCTAssertEqual(SetState(member: member("x", 0, "discarded", dropped: true)), .dropped)
        XCTAssertEqual(SetState.dropped.glyph, "–")
    }

    // MARK: summary

    func testTheSummaryOfEachSetState() {
        XCTAssertEqual(SetText.summary(partlyInMain), "1 of 3 in main")
        XCTAssertEqual(SetText.title(partlyInMain), "Set 8a46cad2 · 1 of 3 in main")
        let building = set([member("api", 0, "merged"), member("web", 1, "running", head: nil), member("worker", 2, "merged")], state: "running")
        XCTAssertEqual(SetText.summary(building), "building · 2 of 3 built")
        XCTAssertEqual(SetText.title(building), "Set 8a46cad2 · building")
        let built = set([member("api", 0, "merged"), member("web", 1, "merged")])
        XCTAssertEqual(SetText.summary(built), "built · none in main yet")
        XCTAssertEqual(SetText.title(built), "Set 8a46cad2 · built")
        let all = set([member("api", 0, "confirmed"), member("web", 1, "confirmed"), member("x", 2, "confirmed")])
        XCTAssertEqual(SetText.summary(all), "in main · all 3 repos")
        let conflicted = set([member("api", 0, "merged"), member("web", 1, "conflicted", head: nil)], state: "conflicted")
        XCTAssertEqual(SetText.summary(conflicted), "conflicted in web")
        let failed = set([member("api", 0, "merged"), member("web", 1, "failed", attempt: 2)], state: "failed")
        XCTAssertEqual(SetText.summary(failed), "failed in web")
        let discarded = set([member("api", 0, "discarded"), member("web", 1, "discarded")], state: "discarded")
        XCTAssertEqual(SetText.summary(discarded), "discarded · nothing reached main")
        let kept = set([member("api", 0, "confirmed"), member("web", 1, "discarded")], state: "discarded")
        XCTAssertEqual(SetText.summary(kept), "discarded · 1 of 2 stay in main")
    }

    func testADroppedMemberIsOutsideTheCounts() {
        let dropped = set([
            member("api", 0, "confirmed"), member("web", 1, "merged"), member("worker", 2, "merged"),
            member("infra", 3, "discarded", dropped: true),
        ])
        XCTAssertEqual(SetText.summary(dropped), "1 of 3 in main")
        let allIn = set([member("api", 0, "confirmed"), member("infra", 1, "discarded", dropped: true)])
        XCTAssertEqual(SetText.summary(allIn), "in main · all 1 repos")
    }

    func testTheReasonASetWaits() {
        XCTAssertEqual(SetText.reason(partlyInMain), "Merge web next")
        let conflicted = set([member("api", 0, "merged"), member("web", 1, "conflicted", head: nil)], state: "conflicted")
        XCTAssertEqual(SetText.reason(conflicted), "web conflicted: redo it or message the PM")
        let failed = set([member("api", 0, "merged"), member("web", 1, "failed", attempt: 2)], state: "failed")
        XCTAssertEqual(SetText.reason(failed), "web failed, attempt 2")
        let failedOnce = set([member("web", 0, "failed")], state: "failed")
        XCTAssertEqual(SetText.reason(failedOnce), "web failed")
        let all = set([member("api", 0, "confirmed")])
        XCTAssertNil(SetText.reason(all))
    }

    // MARK: strip

    func testAShortStripShowsEveryMemberInMergeOrder() {
        let strip = SetText.strip(partlyInMain)
        XCTAssertEqual(strip.text, "●○○")
        XCTAssertEqual(strip.members.map(\.repo), ["api", "web", "worker"])
    }

    func testALongStripIsCappedAtTwelveWithAPlusCount() {
        let members = (0..<32).map { member("r\($0)", Int64($0), "running", head: nil) }
        let long = set(members, repo: "r0", state: "running")
        let strip = SetText.strip(long)
        XCTAssertEqual(strip.members.count, 12)
        XCTAssertEqual(strip.trailing, 20)
        XCTAssertEqual(strip.leading, 0)
        XCTAssertTrue(strip.text.hasSuffix(" +20"))
    }

    func testTheStripStartsTwoBeforeANextRepoBeyondTheCap() {
        var members = (0..<32).map { member("r\($0)", Int64($0), "confirmed") }
        for index in 20..<32 { members[index] = member("r\(index)", Int64(index), "merged") }
        let strip = SetText.strip(set(members, repo: "r20"))
        XCTAssertEqual(strip.members.first?.repo, "r18")
        XCTAssertEqual(strip.leading, 18)
        XCTAssertEqual(strip.members.count, 12)
        XCTAssertEqual(strip.trailing, 2)
        XCTAssertTrue(strip.text.hasPrefix("+18 "))
    }

    // MARK: rows

    func testMemberRowsCarryNextReviewSubjectAttemptAndDropped() {
        let members = [
            member("api", 0, "confirmed"), member("web", 1, "merged", attempt: 2), member("worker", 2, "conflicted", head: nil),
            member("infra", 3, "discarded", dropped: true),
        ]
        let rows = SetMemberRow.rows(set(members, subject: true), verify: [
            RepoVerify(repo: "api", latest: IntegrationVerifyRun(state: "passed", commitSha: "3f2a9c1d5b7e", finishedAt: "t")),
            RepoVerify(repo: "web", latest: IntegrationVerifyRun(state: "queued", commitSha: "3f2a9c1d5b7e", finishedAt: "t")),
        ])
        XCTAssertEqual(rows.map(\.position), [1, 2, 3, 4])
        XCTAssertEqual(rows.map(\.isNext), [false, true, false, false])
        XCTAssertEqual(rows.map(\.isReviewSubject), [false, true, false, false])
        XCTAssertEqual(rows[1].attempt, "attempt 2")
        XCTAssertNil(rows[0].attempt)
        XCTAssertEqual(rows[0].sub, "3f2a9c1 · verify passed")
        XCTAssertEqual(rows[1].sub, "3f2a9c1 · verify running", "a queued run reads as running")
        XCTAssertEqual(rows[2].sub, "no head yet")
        XCTAssertEqual(rows[3].sub, "Outside the gate, review and confirm")
        XCTAssertTrue(rows[3].struck)
        XCTAssertEqual(rows[1].voiceover, "2 of 4, web, built, not in main, review subject, next to merge, attempt 2")
        XCTAssertEqual(rows[3].voiceover, "4 of 4, infra, dropped, outside the gate, review and confirm")
        XCTAssertEqual(rows[1].isCurrent, true)
    }

    func testAMemberWithoutAVerifyRunReadsNotRun() {
        let rows = SetMemberRow.rows(partlyInMain, verify: [])
        XCTAssertEqual(rows[0].sub, "3f2a9c1 · verify not run")
    }

    // MARK: gate and per-repo verify

    private func run(_ state: String) -> IntegrationVerifyRun {
        IntegrationVerifyRun(state: state, commitSha: "3f2a9c1d5b7e", finishedAt: "t")
    }

    func testTheGateFailedNamesTheFailedRepos() {
        let members = [member("api", 0, "merged"), member("web", 1, "merged"), member("worker", 2, "merged")]
        let gate = SetGate.make(
            set(members),
            verify: [
                RepoVerify(repo: "api", latest: run("passed")), RepoVerify(repo: "web", latest: run("failed")),
                RepoVerify(repo: "worker"),
            ])
        XCTAssertEqual(gate?.state, .failed)
        XCTAssertEqual(gate?.line, "gate failed: web")
    }

    func testTheGateWaitingNamesRunningAndMissingRunsAndPassesWhenAllPassed() {
        let members = [member("api", 0, "merged"), member("web", 1, "merged"), member("worker", 2, "merged"),
                       member("infra", 3, "discarded", dropped: true)]
        let waiting = SetGate.make(
            set(members),
            verify: [RepoVerify(repo: "api", latest: run("passed")), RepoVerify(repo: "web", latest: run("queued")),
                     RepoVerify(repo: "worker")])
        XCTAssertEqual(waiting?.line, "gate waiting: web, worker")
        XCTAssertEqual(waiting?.lead, "Every repo’s latest run must pass. 1 passed; 1 is running.")
        let passed = SetGate.make(
            set(members),
            verify: [RepoVerify(repo: "api", latest: run("passed")), RepoVerify(repo: "web", latest: run("passed")),
                     RepoVerify(repo: "worker", latest: run("passed"))])
        XCTAssertEqual(passed?.state, .passed)
        XCTAssertEqual(passed?.line, "gate passed: every repo's latest run passed")
        XCTAssertNil(SetGate.make(set(members), verify: nil), "no per-repo data, no gate")
    }

    func testThePerRepoVerifyLine() {
        let web = member("web", 1, "merged")
        let last = IntegrationVerifyPassed(commitSha: "1111111aaaaa", finishedAt: "t")
        XCTAssertEqual(
            SetGate.repoLine(RepoVerify(repo: "web", latest: run("failed"), lastPassed: last), member: web),
            "web failed · last passed 1111111")
        XCTAssertEqual(SetGate.repoLine(nil, member: member("worker", 2, "merged")), "worker not run")
        XCTAssertEqual(
            SetGate.repoLine(nil, member: member("infra", 3, "discarded", dropped: true)), "infra outside the gate")
    }

    // MARK: VoiceOver

    func testVoiceOver() {
        XCTAssertEqual(SetText.rowVoiceover(partlyInMain), "Set 8a46cad2, 1 of 3 in main, next web")
        XCTAssertEqual(
            SetText.stripVoiceover(partlyInMain),
            "1 of 3 in main. api in main, web built, not in main, worker built, not in main")
        let long = set((0..<15).map { member("r\($0)", Int64($0), "running", head: nil) }, repo: "r0", state: "running")
        XCTAssertTrue(SetText.stripVoiceover(long).hasSuffix(", and 3 more repos"))
    }
}

final class SetRowLayoutTests: XCTestCase {
    private func row(_ key: String, set: IntegrationSet?, summary: String = "merged b") -> InboxRow {
        InboxRow(
            id: InboxRowID(hostId: UUID(), projectId: "p", key: key), kind: .integration, projectName: "p",
            since: "2026-10-01T10:00:00Z", summary: UntrustedText(raw: summary, verified: true, truncated: false),
            needsTerminal: false, forwarded: false, stale: false, requestId: key, contentHash: "h", set: set)
    }

    private var set: IntegrationSet {
        IntegrationSet(
            setId: "8a46cad2", state: "merged", repo: "web", position: 1,
            members: [
                SetMember(repo: "api", position: 0, state: "confirmed", headSha: "3f2a9c1d5b7e"),
                SetMember(repo: "web", position: 1, state: "merged", headSha: "3f2a9c1d5b7e"),
                SetMember(repo: "worker", position: 2, state: "merged", headSha: "3f2a9c1d5b7e"),
            ])
    }

    func testASetRowHasItsOwnTitleAndSecondLine() {
        let setRow = row("integration:web", set: set)
        XCTAssertEqual(RowText.title(setRow), "Set 8a46cad2 · 1 of 3 in main")
        XCTAssertEqual(RowText.secondary(setRow), .plain("●○○  Merge web next"))
    }

    func testSetRowsGoToTheirOwnGroupAndSingleRepoRowsStay() {
        let single = row("integration:solo", set: nil, summary: "merged b")
        let groups = InboxLayout.groups([
            InboxGroup(kind: .integration, title: "x", rows: [single, row("integration:web", set: set)])
        ])
        XCTAssertEqual(groups.map(\.title), ["Ready to merge", "Integration sets"])
        XCTAssertEqual(groups[0].rows.map(\.id.key), ["integration:solo"])
        XCTAssertEqual(groups[1].rows.map(\.id.key), ["integration:web"])
        XCTAssertEqual(groups[1].header, "Integration sets · 1")
    }

    func testAConflictedSetStaysInTheSetGroupNotInMergeConflicts() {
        let conflicted = IntegrationSet(
            setId: "s", state: "conflicted", repo: "web", position: 0,
            members: [SetMember(repo: "web", position: 0, state: "conflicted")])
        let groups = InboxLayout.groups([
            InboxGroup(kind: .integration, title: "x", rows: [row("integration:web", set: conflicted, summary: "conflicted b")])
        ])
        XCTAssertEqual(groups.map(\.title), ["Integration sets"])
    }
}
