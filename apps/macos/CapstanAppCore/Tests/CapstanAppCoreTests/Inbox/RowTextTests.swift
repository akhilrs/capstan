import CapstanKit
import XCTest

@testable import CapstanAppCore

final class RowTextTests: XCTestCase {
    private let host = UUID(uuidString: "A1A1A1A1-0000-4000-8000-000000000001")!

    private func row(
        _ kind: WaitingKind, key: String = "k", project: String = "payments-api", summary: String = "text",
        verified: Bool = true, since: String = "2026-10-29T08:00:00Z"
    ) -> InboxRow {
        InboxRow(
            id: InboxRowID(hostId: host, projectId: "p", key: key), kind: kind, projectName: project, since: since,
            summary: UntrustedText(raw: summary, verified: verified, truncated: false), needsTerminal: false,
            forwarded: false, stale: false, requestId: "r", contentHash: "aaaaaaaaaaaa")
    }

    func test_Title_perKind() {
        XCTAssertEqual(RowText.title(row(.op)), "payments-api · operator")
        XCTAssertEqual(RowText.title(row(.prompt)), "payments-api · an agent asks for permission")
        XCTAssertEqual(RowText.title(row(.design, key: "design:8/onboarding")), "payments-api · plan-8 / onboarding")
        XCTAssertEqual(RowText.title(row(.design, key: "odd")), "payments-api · design approval")
        XCTAssertEqual(RowText.title(row(.message)), "payments-api · needs an answer")
        XCTAssertEqual(
            RowText.title(row(.integration, summary: "merged integration/plan-12")), "payments-api · integration/plan-12")
        XCTAssertEqual(RowText.title(row(.integration, summary: "merged")), "payments-api · integration")
    }

    func test_Title_cutsALongProjectName() {
        let long = String(repeating: "x", count: 200)
        XCTAssertLessThanOrEqual(RowText.title(row(.op, project: long)).utf8.count, RowText.projectCap + " · operator".utf8.count)
    }

    func test_Secondary() {
        XCTAssertEqual(RowText.secondary(row(.op, summary: "git push origin main")), .mono("git push origin main"))
        XCTAssertEqual(RowText.secondary(row(.prompt, summary: "Run it?")), .plain("Run it?"))
        XCTAssertEqual(RowText.secondary(row(.op, summary: "x", verified: false)), .unverified)
        XCTAssertEqual(RowText.secondary(row(.integration, summary: "conflicted b")), .plain("conflicted"))
        XCTAssertEqual(RowText.secondary(row(.integration, summary: "merged b")), .plain("Merged, ready for you to review"))
    }

    func test_IntegrationState() {
        XCTAssertEqual(RowText.integrationState(row(.integration, summary: "merged feat/a")), .merged)
        XCTAssertEqual(RowText.integrationState(row(.integration, summary: "conflicted feat/a")), .conflicted)
        XCTAssertEqual(RowText.integrationState(row(.integration, summary: "weird feat/a")), .other)
        XCTAssertEqual(RowText.branch(row(.integration, summary: "merged feat/a b")), "feat/a b")
    }

    func test_Groups_splitMergedFromConflicted() {
        let merged = row(.integration, key: "integration:1", summary: "merged feat/a")
        let conflicted = row(.integration, key: "integration:2", summary: "conflicted feat/b")
        let op = row(.op, key: "op:1")
        let groups = InboxLayout.groups([
            InboxGroup(kind: .op, title: "x", rows: [op]),
            InboxGroup(kind: .integration, title: "y", rows: [conflicted, merged]),
        ])
        XCTAssertEqual(groups.map(\.title), ["Command proposals", "Ready to merge", "Merge conflicts"])
        XCTAssertEqual(groups[1].rows, [merged])
        XCTAssertEqual(groups[2].rows, [conflicted])
        XCTAssertEqual(groups[1].header, "Ready to merge · 1")
        XCTAssertFalse(groups[1].quiet)
        XCTAssertTrue(groups[2].quiet)
    }

    func test_Groups_onlyConflictedLeavesNoReadyGroup() {
        let conflicted = row(.integration, summary: "conflicted feat/b")
        let groups = InboxLayout.groups([InboxGroup(kind: .integration, title: "y", rows: [conflicted])])
        XCTAssertEqual(groups.map(\.title), ["Merge conflicts"])
        XCTAssertEqual(groups[0].id, "merge-conflicts")
    }

    func test_Groups_emptyInputIsEmpty() {
        XCTAssertTrue(InboxLayout.groups([]).isEmpty)
    }

    func test_KindTab_roundTrip() {
        for tab in KindTab.allCases where tab != .merges {
            XCTAssertEqual(KindTab(kind: tab.kind), tab)
        }
        XCTAssertEqual(KindTab(kind: .integration), .merges)
        XCTAssertEqual(KindTab.visible.map(\.title), ["All", "Proposals", "Prompts", "Designs"])
        XCTAssertEqual(KindTab.more.map(\.title), ["Messages", "Merges"])
    }

    func test_FooterStatus_oneHostLive() {
        let now = Date(timeIntervalSince1970: 1_000)
        let live = ConnectionState.live(since: now.addingTimeInterval(-60), lastLineAt: now.addingTimeInterval(-3))
        let status = FooterStatus.make([.init(name: "build-host", state: live, loading: false)], now: now)
        XCTAssertEqual(status.text, "build-host live · heartbeat 3 s ago")
        XCTAssertEqual(status.tone, .green)
    }

    func test_FooterStatus_severalHosts() {
        let now = Date(timeIntervalSince1970: 1_000)
        let live = ConnectionState.live(since: now, lastLineAt: now)
        let allLive = FooterStatus.make(
            [.init(name: "a", state: live, loading: false), .init(name: "b", state: live, loading: false)], now: now)
        XCTAssertEqual(allLive.text, "2 hosts live")
        let retry = ConnectionState.reconnecting(attempt: 1, retryAt: now, lastError: nil)
        let mixed = FooterStatus.make(
            [.init(name: "a", state: live, loading: false), .init(name: "studio-mini", state: retry, loading: false)],
            now: now)
        XCTAssertEqual(mixed.text, "studio-mini reconnecting · try 1 in 0 s")
        XCTAssertEqual(mixed.name, "studio-mini")
        XCTAssertTrue(mixed.ring)
        XCTAssertEqual(mixed.tone, .yellow)
        XCTAssertEqual(FooterStatus.make([], now: now).text, "No hosts")
    }

    func test_StoppedHostIsRedAfterAFailureAndGreyWhenTheUserStoppedIt() {
        let failed = ConnectionState.stopped(.repeatedFailure)
        XCTAssertEqual(SidebarText.hostStatus(state: failed, projects: []).tone, .red)
        XCTAssertEqual(SidebarText.hostStatus(state: .stopped(.user), projects: []).tone, .grey)
        let status = FooterStatus.make([.init(name: "a", state: failed, loading: false)], now: Date())
        XCTAssertEqual(status.tone, .red)
        XCTAssertEqual(status.text, "a stopped")
    }

    func test_SidebarText() {
        XCTAssertNil(SidebarText.projectStatus(.ready))
        XCTAssertEqual(SidebarText.projectStatus(.unsupported), "unsupported")
        XCTAssertEqual(SidebarText.hostStatus(state: nil, projects: []).word, "offline")
        XCTAssertEqual(SidebarText.hostStatus(state: .connecting, projects: []).word, "connecting")
    }

    func test_InboxScope_search() {
        let a = row(.op, project: "payments-api", summary: "git push origin main")
        let b = row(.op, key: "op:2", project: "docs-site", summary: "rm -rf build")
        let groups = [InboxGroup(kind: .op, title: "x", rows: [a, b])]
        XCTAssertEqual(InboxScope(search: "DOCS").apply(groups).flatMap(\.rows), [b])
        XCTAssertEqual(InboxScope(search: "push").apply(groups).flatMap(\.rows), [a])
        XCTAssertEqual(InboxScope(search: "  ").apply(groups).flatMap(\.rows).count, 2)
        XCTAssertTrue(InboxScope(search: "zzz").apply(groups).isEmpty)
    }
}
