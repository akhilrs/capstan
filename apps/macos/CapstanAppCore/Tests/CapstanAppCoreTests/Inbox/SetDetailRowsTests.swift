import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class SetDetailRowsTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_790_000_000)

    private func run(_ state: String) -> IntegrationVerifyRun {
        IntegrationVerifyRun(state: state, commitSha: "3f2a9c1d5b7e", finishedAt: "t")
    }

    private func item(members: [SetMember], repo: String, byRepo: [RepoVerify]?, state: String = "merged") -> StoredWaiting {
        let set = IntegrationSet(
            setId: "8a46cad2", state: state, repo: repo, position: members.first { $0.repo == repo }?.position ?? 0,
            reviewSubject: true, members: members)
        let details = IntegrationDetails(recorded: false, set: set, byRepo: byRepo)
        return StoredWaiting(
            key: "integration:1", requestId: "integration:1", kind: .integration, since: "2026-10-29T08:00:00Z",
            summary: UntrustedText(raw: "merged integration/set-4", verified: true, truncated: false),
            contentHash: "h", needsTerminal: false, integrationState: "merged",
            branch: UntrustedText(raw: "integration/set-4", verified: true, truncated: false), headSha: "3f2a9c1d5b7e",
            integrationDetails: details, set: set)
    }

    private func blocks(_ rows: IntegrationDetailRows) -> [DetailBlock] {
        rows.sections.flatMap(\.blocks) + rows.banners
    }

    func testTheInspectorListsTheMembersInMergeOrderFirst() {
        let members = [
            SetMember(repo: "api", position: 0, state: "confirmed", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "web", position: 1, state: "merged", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "infra", position: 2, state: "discarded", dropped: true),
        ]
        let rows = IntegrationDetailRows.make(
            item(members: members, repo: "web", byRepo: [RepoVerify(repo: "api", latest: run("passed"))]),
            hostName: "h", now: now)
        XCTAssertEqual(rows.title, "Set 8a46cad2 · 1 of 2 in main")
        XCTAssertEqual(rows.sections.first?.title, "Repos in merge order")
        XCTAssertEqual(rows.sections.first?.summary, "2 repos · 1 dropped")
        guard case .setMembers(_, let count, let list)? = rows.sections.first?.blocks.first else {
            return XCTFail("members block")
        }
        XCTAssertEqual(count, "2 repos · 1 dropped")
        XCTAssertEqual(list.map(\.repo), ["api", "web", "infra"])
        XCTAssertEqual(list.map(\.isNext), [false, true, false])
        XCTAssertEqual(list.map(\.struck), [false, false, true])
    }

    func testTheVerifySectionHasOneResultPerRepoAndTheGateBanner() throws {
        let members = [
            SetMember(repo: "api", position: 0, state: "merged", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "web", position: 1, state: "merged", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "worker", position: 2, state: "merged", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "infra", position: 3, state: "discarded", dropped: true),
        ]
        let last = IntegrationVerifyPassed(commitSha: "1111111aaaaa", finishedAt: "t")
        let rows = IntegrationDetailRows.make(
            item(
                members: members, repo: "web",
                byRepo: [
                    RepoVerify(repo: "api", latest: run("passed")),
                    RepoVerify(repo: "web", latest: run("failed"), lastPassed: last), RepoVerify(repo: "worker"),
                ]),
            hostName: "h", now: now)
        let verify = try XCTUnwrap(rows.sections.first { $0.title == "Verify by repo" })
        guard case .results(let results)? = verify.blocks.first else { return XCTFail("results") }
        XCTAssertEqual(results.map(\.label), ["api", "web", "worker", "infra"])
        XCTAssertEqual(results.map(\.word), ["passed", "failed", "not run", "outside the gate"])
        XCTAssertEqual(results[1].notes, ["last passed 1111111"])
        XCTAssertTrue(
            rows.banners.contains { if case .banner(_, let title, _) = $0 { return title == "gate failed: web" } else { return false } })
    }

    func testAConflictedMemberOffersTheRedoCommand() {
        let members = [
            SetMember(repo: "api", position: 0, state: "merged", headSha: "3f2a9c1d5b7e"),
            SetMember(repo: "web", position: 1, state: "conflicted"),
        ]
        let rows = IntegrationDetailRows.make(
            item(members: members, repo: "web", byRepo: nil, state: "conflicted"), hostName: "h", now: now)
        XCTAssertEqual(rows.lead, "Nothing reached main. The other repos keep their work.")
        XCTAssertTrue(
            blocks(rows).contains {
                if case .banner(_, let title, let text) = $0 {
                    return title == "web conflicted: redo it or message the PM"
                        && text == "cstan integrate redo 8a46cad2 --repo web"
                }
                return false
            })
    }

    func testAFailedRedoSaysAttemptAndOffersDiscard() {
        let members = [SetMember(repo: "web", position: 0, attempt: 2, state: "failed")]
        let rows = IntegrationDetailRows.make(
            item(members: members, repo: "web", byRepo: nil, state: "failed"), hostName: "h", now: now)
        XCTAssertEqual(rows.lead, "The redo failed too. Discard web to go on without it, or message the PM.")
        XCTAssertTrue(
            blocks(rows).contains {
                if case .banner(_, let title, let text) = $0 {
                    return title == "web failed, attempt 2" && text == "cstan integrate discard 8a46cad2 --repo web"
                }
                return false
            })
    }

    func testAMergeStepNamesTheRepo() {
        let members = [SetMember(repo: "web", position: 0, state: "merged", headSha: "3f2a9c1d5b7e")]
        let rows = IntegrationDetailRows.make(
            item(members: members, repo: "web", byRepo: nil), hostName: "h", now: now, mergeAvailable: true)
        XCTAssertEqual(rows.steps.first?.title, "Merge web into main")
        XCTAssertEqual(rows.steps.first?.action, .merge)
    }

    func testAnItemWithoutASetKeepsTheShippedRows() {
        let plain = StoredWaiting(
            key: "integration:7", requestId: "r", kind: .integration, since: "2026-10-29T08:00:00Z",
            summary: UntrustedText(raw: "merged integration/plan-12", verified: true, truncated: false),
            contentHash: "h", needsTerminal: false, integrationState: "merged",
            branch: UntrustedText(raw: "integration/plan-12", verified: true, truncated: false), headSha: "7c41e9a02d5f",
            integrationDetails: IntegrationDetails(recorded: false))
        let rows = IntegrationDetailRows.make(plain, hostName: "h", now: now)
        XCTAssertEqual(rows.title, "integration/plan-12 is built, not in main yet")
        XCTAssertFalse(rows.sections.contains { $0.title == "Repos in merge order" })
    }
}
