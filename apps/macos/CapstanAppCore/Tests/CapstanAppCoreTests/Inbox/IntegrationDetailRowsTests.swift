import CapstanKit
import XCTest

@testable import CapstanAppCore

final class IntegrationDetailRowsTests: XCTestCase {
    private let branch = "integration/plan-12"
    private let head = "7c41e9a02d5f"

    private var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }

    private var now: Date { MenuList.date("2026-10-29T14:00:00Z")! }

    private func text(_ raw: String, verified: Bool = true, truncated: Bool = false) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: truncated)
    }

    private func item(
        state: String = "merged", branch: UntrustedText? = nil, head: String? = "7c41e9a02d5f",
        details: IntegrationDetails?
    ) -> StoredWaiting {
        let name = branch ?? text("integration/plan-12")
        return StoredWaiting(
            key: "integration:7", requestId: "integration:int-7", kind: .integration, since: "2026-10-29T08:00:00Z",
            summary: text(state + " integration/plan-12"), contentHash: "aaaaaaaaaaaa", needsTerminal: false,
            integrationState: state, branch: name, headSha: head, integrationDetails: details)
    }

    private func rows(_ stored: StoredWaiting, merge: Bool = false) -> IntegrationDetailRows {
        IntegrationDetailRows.make(stored, hostName: "build-host", now: now, mergeAvailable: merge, calendar: utc)
    }

    private func section(_ made: IntegrationDetailRows, _ title: String) throws -> DetailSection {
        try XCTUnwrap(made.sections.first { $0.title == title }, title)
    }

    private func entries(_ block: DetailBlock) -> (label: String, entries: [DetailEntry], more: String?)? {
        if case .entries(let label, let list, let more) = block { return (label, list, more) }
        return nil
    }

    private func notes(_ section: DetailSection) -> [String] {
        section.blocks.compactMap { block -> String? in
            if case .note(let line) = block { return line }
            return nil
        }
    }

    private func recorded(_ details: IntegrationDetails) -> IntegrationDetailRows {
        rows(item(details: details))
    }

    // MARK: state

    func test_StateWords_neverTheBareWordMerged() {
        let built = rows(item(details: nil))
        XCTAssertEqual(built.stateWord, "Built · not in main")
        XCTAssertEqual(built.stateTone, .blue)
        XCTAssertEqual(built.title, "integration/plan-12 is built, not in main yet")
        let conflicted = rows(item(state: "conflicted", details: IntegrationDetails(recorded: false)))
        XCTAssertEqual(conflicted.stateWord, "Conflicted · nothing to merge")
        XCTAssertEqual(conflicted.stateTone, .yellow)
        XCTAssertEqual(conflicted.title, "integration/plan-12 has a conflict")
        let other = rows(item(state: "weird", details: nil))
        for made in [built, conflicted, other] {
            XCTAssertNotEqual(made.stateWord.lowercased(), "merged")
            XCTAssertFalse(made.title.lowercased().hasSuffix("merged"))
        }
        XCTAssertEqual(other.stateWord, "Integration")
    }

    func test_State_fallsBackToTheSummaryOfAnOlderHost() {
        var stored = item(details: nil)
        stored.integrationState = nil
        stored.branch = nil
        let made = rows(stored)
        XCTAssertEqual(made.stateWord, "Built · not in main")
        XCTAssertEqual(made.meta.first?.value.raw, "integration/plan-12")
    }

    func test_Header_metaAndLead() {
        let made = recorded(
            IntegrationDetails(
                recorded: true, reports: [IntegrationReport(reportId: "r-1", summary: text("a"))], reportsOmitted: 2))
        XCTAssertEqual(made.meta.map(\.label), ["Branch", "Head"])
        XCTAssertEqual(made.meta.last?.value.raw, "7c41e9a02d5f")
        XCTAssertEqual(
            made.lead,
            "Its 3 reports merged cleanly into the integration branch. The project's main branch does not have them until "
                + "the branch is merged.")
        XCTAssertEqual(made.stateWord, "Built · not in main")
    }

    func test_Header_noHeadRowWithoutASha_andLongShaIsCutToTwelve() {
        let none = rows(item(head: nil, details: nil))
        XCTAssertEqual(none.meta.map(\.label), ["Branch"])
        let long = rows(item(head: String(repeating: "a", count: 40), details: nil))
        XCTAssertEqual(long.meta.last?.value.raw.count, 12)
    }

    func test_Lead_oneReportAndNoCountWithoutDetails() {
        let one = recorded(IntegrationDetails(recorded: true, reports: [IntegrationReport(reportId: "r", summary: text("a"))]))
        XCTAssertTrue(one.lead?.hasPrefix("Its 1 report merged cleanly") ?? false)
        XCTAssertTrue(rows(item(details: nil)).lead?.hasPrefix("Its reports merged cleanly") ?? false)
        XCTAssertNil(rows(item(state: "conflicted", details: nil)).lead)
    }

    func test_Conflicted_bannerAndNotRecordedNote() throws {
        let made = rows(item(state: "conflicted", details: IntegrationDetails(recorded: false)))
        XCTAssertEqual(
            made.banners.first,
            .banner(
                tone: .yellow, title: "The reports conflict with each other",
                text: "Building the integration branch stopped at a conflict. Nothing reached main and nothing is ready to "
                    + "merge."))
        let body = try section(made, "What it is for · Impact · Results")
        XCTAssertEqual(
            notes(body),
            [
                "Not recorded. Capstan records these only when an integration merges cleanly; a conflicted one never has "
                    + "them. This is expected."
            ])
    }

    func test_Conflicted_stepsHaveNoMerge() {
        let made = rows(item(state: "conflicted", details: IntegrationDetails(recorded: false)), merge: true)
        XCTAssertEqual(
            made.steps.map { $0.title },
            ["The project manager has a developer resolve it", "A new integration replaces this one"])
        XCTAssertTrue(made.steps.allSatisfy { $0.command == nil && $0.action == nil })
    }

    func test_NoDetails_fromAHostThatDoesNotSendThem() {
        let made = rows(item(details: nil))
        XCTAssertTrue(made.sections.isEmpty)
        XCTAssertEqual(
            made.banners,
            [
                .banner(
                    tone: .grey, title: "No details from this host",
                    text: "This host's Capstan does not send integration details. Update Capstan on build-host to see "
                        + "purpose, impact and results. The next steps below still apply.")
            ])
        XCTAssertEqual(made.steps.count, 2)
    }

    func test_NotRecorded_builtBeforeDetailsWereKept() {
        let made = rows(item(details: IntegrationDetails(recorded: false)))
        XCTAssertTrue(made.sections.isEmpty)
        XCTAssertEqual(
            made.banners,
            [
                .banner(
                    tone: .grey, title: "Details not recorded",
                    text: "This integration was built before Capstan kept its purpose, impact and results. Run cstan "
                        + "integrate show on the host to see its reports.")
            ])
        XCTAssertEqual(made.steps.count, 2, "the terminal steps, with no host steps")
    }

    func test_RecordedDetails_makeThreeSectionsInOrder() {
        let made = recorded(IntegrationDetails(recorded: true))
        XCTAssertEqual(made.sections.map(\.title), ["What it is for", "Impact", "Results"])
        XCTAssertTrue(made.banners.isEmpty)
    }

    // MARK: what it is for

    func test_Plans_oneLinePerPlan_withOmittedAndEmpty() throws {
        let plans = [IntegrationPlan(planId: "plan-12", title: text("Search"))]
        let one = try section(recorded(IntegrationDetails(recorded: true, plans: plans)), "What it is for")
        let block = try XCTUnwrap(entries(one.blocks[0]))
        XCTAssertEqual(block.label, "Plan · untrusted title")
        XCTAssertEqual(block.entries.first?.caption, "plan-12")
        XCTAssertEqual(block.entries.first?.shown, "Search")
        XCTAssertNil(block.more)
        let many = try section(
            recorded(IntegrationDetails(recorded: true, plans: plans, plansOmitted: 2)), "What it is for")
        let manyBlock = try XCTUnwrap(entries(many.blocks[0]))
        XCTAssertEqual(manyBlock.label, "Plans · untrusted titles")
        XCTAssertEqual(manyBlock.more, "and 2 more plans")
        let empty = try section(recorded(IntegrationDetails(recorded: true)), "What it is for")
        XCTAssertEqual(empty.blocks[0], .note("No plan: these reports were integrated outside a plan."))
    }

    func test_Signoffs_captionTimeAndPlanNotInTheIntegration() throws {
        let plans = [IntegrationPlan(planId: "plan-15", title: text("Ledger"))]
        let signoffs = [
            IntegrationSignoff(planId: "plan-15", summary: text("Reviewed"), createdAt: "2026-10-29T12:40:00.000Z"),
            IntegrationSignoff(planId: "plan-11", summary: text("Superseded"), createdAt: "2026-10-24T09:00:00Z"),
            IntegrationSignoff(planId: "plan-15", summary: text("Earlier"), createdAt: "2026-10-28T17:05:00Z"),
        ]
        let details = IntegrationDetails(recorded: true, plans: plans, signoffs: signoffs, signoffsOmitted: 1)
        let body = try section(recorded(details), "What it is for")
        let block = try XCTUnwrap(entries(body.blocks[1]))
        XCTAssertEqual(block.label, "Architect sign-off · untrusted text")
        XCTAssertEqual(
            block.entries.map { $0.caption },
            [
                "plan-15 · today 12:40", "plan-11 · Oct 24 · not in this integration's plans",
                "plan-15 · yesterday 17:05",
            ])
        XCTAssertEqual(block.more, "and 1 more sign-off")
        let none = try section(recorded(IntegrationDetails(recorded: true)), "What it is for")
        XCTAssertEqual(none.blocks[1], .note("Not signed off yet."))
    }

    func test_Reports_captionPackageAndAbsentCases() throws {
        let reports = [
            IntegrationReport(reportId: "r-118", packageId: "search-index", summary: text("Pagefind")),
            IntegrationReport(reportId: "r-121", packageId: nil, summary: text("Outside a plan")),
        ]
        let details = IntegrationDetails(recorded: true, reports: reports, reportsOmitted: 3)
        let body = try section(recorded(details), "What it is for")
        let block = try XCTUnwrap(entries(body.blocks[2]))
        XCTAssertEqual(block.label, "Reports · untrusted summaries")
        XCTAssertEqual(block.entries.map { $0.caption }, ["r-118 · search-index", "r-121 · no package"])
        XCTAssertEqual(block.more, "and 3 more reports")
        XCTAssertEqual(body.summary, "no plan · 5 reports · not signed off")
        let none = try section(recorded(IntegrationDetails(recorded: true)), "What it is for")
        XCTAssertEqual(none.blocks[2], .note("No reports listed."))
    }

    func test_UntrustedText_keepsItsFlags() throws {
        let cut = text("Reviewed all five packages", truncated: true)
        let bad = text("secret", verified: false)
        let reports = [
            IntegrationReport(reportId: "r-1", summary: cut), IntegrationReport(reportId: "r-2", summary: bad),
        ]
        let plans = [IntegrationPlan(planId: "plan-1", title: bad)]
        let body = try section(recorded(IntegrationDetails(recorded: true, plans: plans, reports: reports)), "What it is for")
        let reportBlock = try XCTUnwrap(entries(body.blocks[2]))
        XCTAssertEqual(reportBlock.entries[0].shown, "Reviewed all five packages…")
        XCTAssertEqual(reportBlock.entries[0].cutNote, "Cut by the host. The full summary is on the host.")
        XCTAssertNil(reportBlock.entries[0].notShown)
        XCTAssertNil(reportBlock.entries[1].shown)
        XCTAssertEqual(
            reportBlock.entries[1].notShown, "Summary not shown: it failed its integrity check. Read it in the terminal.")
        let planBlock = try XCTUnwrap(entries(body.blocks[0]))
        XCTAssertEqual(planBlock.entries[0].notShown, "Title not shown: it failed its integrity check.")
        XCTAssertNil(planBlock.entries[0].cutNote)
    }

    // MARK: impact

    func test_Impact_statLine() throws {
        let impact = IntegrationImpact(filesChanged: 23, insertions: 1284, deletions: 96)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        XCTAssertEqual(body.blocks[0], .stat(files: "23 files changed", added: "+1,284", removed: "−96"))
        XCTAssertEqual(body.summary, "23 files · +1,284 −96")
        XCTAssertEqual(body.blocks[1], .note("Migrations: none."))
    }

    func test_Impact_zeroCountsAndOneFile() throws {
        let impact = IntegrationImpact(filesChanged: 1, insertions: 0, deletions: 0)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        XCTAssertEqual(body.blocks[0], .stat(files: "1 file changed", added: "+0", removed: "−0"))
    }

    func test_Impact_truncatedDiffIsALowerBound() throws {
        let impact = IntegrationImpact(filesChanged: 500, insertions: 18402, deletions: 7115, truncated: true)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        XCTAssertEqual(body.blocks[0], .stat(files: "At least 500 files changed", added: "+18,402", removed: "−7,115"))
        XCTAssertEqual(
            body.blocks[1], .note("The diff was too large to count in full; these are the counts up to where the host stopped."))
        XCTAssertEqual(body.summary, "at least 500 files · +18,402 −7,115")
    }

    func test_Impact_areasWithCountsAndOmitted() throws {
        let areas = [
            IntegrationArea(path: text("site/search/"), files: 9), IntegrationArea(path: text("docs/"), files: 1200),
        ]
        let impact = IntegrationImpact(filesChanged: 3, insertions: 1, deletions: 1, areas: areas, areasOmitted: 14)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        let block = try XCTUnwrap(entries(body.blocks[1]))
        XCTAssertEqual(block.label, "Areas · untrusted paths")
        XCTAssertEqual(block.entries.map { $0.shown }, ["site/search/", "docs/"])
        XCTAssertEqual(block.entries.map { $0.count }, ["9", "1,200"])
        XCTAssertEqual(block.more, "and 14 more areas")
    }

    func test_Impact_noAreasMeansNoBlock() throws {
        let impact = IntegrationImpact(filesChanged: 3, insertions: 1, deletions: 1)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        XCTAssertNil(body.blocks.compactMap { entries($0) }.first { $0.label.hasPrefix("Areas") })
    }

    func test_Impact_migrationsListAndOmitted() throws {
        let migrations = [
            IntegrationMigration(path: text("db/0041.sql")), IntegrationMigration(path: text("db/0042.sql")),
        ]
        let impact = IntegrationImpact(
            filesChanged: 3, insertions: 1, deletions: 1, migrations: migrations, migrationsOmitted: 1)
        let body = try section(recorded(IntegrationDetails(recorded: true, impact: impact)), "Impact")
        let block = try XCTUnwrap(entries(body.blocks[1]))
        XCTAssertEqual(block.label, "Migrations · 3 · untrusted paths")
        XCTAssertEqual(block.entries.map { $0.shown }, ["db/0041.sql", "db/0042.sql"])
        XCTAssertEqual(block.more, "and 1 more migration")
    }

    func test_Impact_errorTimeoutAndDiffFailed() throws {
        let timeout = try section(recorded(IntegrationDetails(recorded: true, impactError: "timeout")), "Impact")
        XCTAssertEqual(
            timeout.blocks,
            [
                .banner(
                    tone: .grey, title: "Impact not measured: the diff timed out",
                    text: "The host stopped measuring the diff after its time limit. The merge itself is not affected.")
            ])
        XCTAssertEqual(timeout.summary, "not measured")
        let failed = try section(recorded(IntegrationDetails(recorded: true, impactError: "diff_failed")), "Impact")
        XCTAssertEqual(
            failed.blocks,
            [
                .banner(
                    tone: .grey, title: "Impact not measured: git could not diff",
                    text: "git failed to compare the branch with main on the host. Run git diff --stat "
                        + "HEAD...integration/plan-12 there to see it.")
            ])
    }

    func test_Impact_nilWithoutAnErrorIsNotRecorded() throws {
        let body = try section(recorded(IntegrationDetails(recorded: true)), "Impact")
        XCTAssertEqual(body.blocks, [.note("Impact not recorded.")])
        XCTAssertEqual(body.summary, "not recorded")
    }

    // MARK: results

    private func results(_ details: IntegrationDetails) throws -> [DetailResult] {
        let body = try section(recorded(details), "Results")
        guard case .results(let list) = body.blocks[0] else {
            XCTFail("results block")
            return []
        }
        return list
    }

    func test_Review_everyVerdictAndNull() throws {
        let cases: [(String, String, Tone)] = [
            ("passed", "Passed", .green), ("findings", "Findings", .yellow), ("failed", "Failed", .red),
            ("cancelled", "Cancelled", .grey),
        ]
        for (verdict, word, tone) in cases {
            let review = IntegrationReview(verdict: verdict, reviewerAgentId: "reviewer-2", round: 2)
            let made = try results(IntegrationDetails(recorded: true, review: review))[0]
            XCTAssertEqual(made.label, "Review")
            XCTAssertEqual(made.word, word, verdict)
            XCTAssertEqual(made.tone, tone, verdict)
            XCTAssertEqual(made.detail, "reviewer-2 · round 2")
        }
        let none = try results(IntegrationDetails(recorded: true))[0]
        XCTAssertEqual(none, DetailResult(label: "Review", word: "Not reviewed yet", tone: .grey, detail: nil, notes: []))
    }

    func test_Verify_neverRun() throws {
        let made = try results(IntegrationDetails(recorded: true))[1]
        XCTAssertEqual(made.word, "Never run")
        XCTAssertEqual(made.tone, .grey)
        XCTAssertEqual(made.detail, "no verify has finished for this branch")
    }

    func test_Verify_onThisCommit_everyState() throws {
        let cases: [(String, String, Tone)] = [
            ("passed", "Passed", .green), ("failed", "Failed", .red), ("timeout", "Timed out", .red),
            ("error", "Error", .red),
        ]
        for (state, word, tone) in cases {
            let latest = IntegrationVerifyRun(state: state, commitSha: head, finishedAt: "2026-10-29T12:00:00Z")
            let made = try results(IntegrationDetails(recorded: true, verify: IntegrationVerify(latest: latest)))[1]
            XCTAssertEqual(made.word, word, state)
            XCTAssertEqual(made.tone, tone, state)
            XCTAssertEqual(made.detail, "on this commit · 2 h ago")
            XCTAssertEqual(made.notes, [])
        }
    }

    func test_Verify_notRunOnThisCommit_latestAndLastPassed() throws {
        let latest = IntegrationVerifyRun(state: "failed", commitSha: "9a1c03e7b4d2", finishedAt: "2026-10-29T11:00:00Z")
        let passed = IntegrationVerifyPassed(commitSha: "5d2e88f1a0c9", finishedAt: "2026-10-28T17:05:00Z")
        let verify = IntegrationVerify(latest: latest, lastPassed: passed)
        let made = try results(IntegrationDetails(recorded: true, verify: verify))[1]
        XCTAssertEqual(made.word, "Not run on this commit")
        XCTAssertEqual(made.tone, .grey)
        XCTAssertNil(made.detail)
        XCTAssertEqual(
            made.notes, ["Latest: Failed on 9a1c03e7b4d2 · 3 h ago", "Last passed on 5d2e88f1a0c9 · yesterday 17:05"])
    }

    func test_Verify_lastPassedIsHiddenWhenItIsTheLatestRun() throws {
        let latest = IntegrationVerifyRun(state: "passed", commitSha: head, finishedAt: "2026-10-29T12:00:00Z")
        let same = IntegrationVerifyPassed(commitSha: head, finishedAt: "2026-10-29T12:00:00Z")
        let made = try results(IntegrationDetails(recorded: true, verify: IntegrationVerify(latest: latest, lastPassed: same)))[1]
        XCTAssertEqual(made.notes, [])
        let older = IntegrationVerifyPassed(commitSha: "1111111aaaa", finishedAt: "2026-10-20T12:00:00Z")
        let shown = try results(IntegrationDetails(recorded: true, verify: IntegrationVerify(latest: latest, lastPassed: older)))[1]
        XCTAssertEqual(shown.notes, ["Last passed on 1111111aaaa · Oct 20"])
    }

    func test_Verify_withoutAHeadShaNeverCountsAsThisCommit() throws {
        let latest = IntegrationVerifyRun(state: "passed", commitSha: head, finishedAt: "2026-10-29T12:00:00Z")
        let details = IntegrationDetails(recorded: true, verify: IntegrationVerify(latest: latest))
        let made = IntegrationDetailRows.make(
            item(head: nil, details: details), hostName: "h", now: now, calendar: utc)
        guard case .results(let list)? = made.sections.last?.blocks.first else { return XCTFail("results") }
        XCTAssertEqual(list[1].word, "Not run on this commit")
    }

    func test_Results_summaryAndFootnote() throws {
        let review = IntegrationReview(verdict: "passed", reviewerAgentId: "r", round: 1)
        let latest = IntegrationVerifyRun(state: "passed", commitSha: head, finishedAt: "2026-10-29T12:00:00Z")
        let details = IntegrationDetails(recorded: true, review: review, verify: IntegrationVerify(latest: latest))
        let body = try section(recorded(details), "Results")
        XCTAssertEqual(body.summary, "review passed · verify passed")
        XCTAssertEqual(
            body.blocks[1],
            .note("Finished results only. A review in progress and a queued or running verify are not shown."))
        let empty = try section(recorded(IntegrationDetails(recorded: true)), "Results")
        XCTAssertEqual(empty.summary, "not reviewed · never verified")
    }

    // MARK: next steps

    func test_Steps_terminalCommandsWithoutAMergeAction() {
        let step = IntegrationDetails(
            recorded: true,
            impact: IntegrationImpact(
                filesChanged: 1, insertions: 1, deletions: 1,
                nextSteps: [IntegrationNextStep(text: text("Run npm ci", truncated: true))]))
        let made = recorded(step)
        XCTAssertEqual(made.steps.count, 3)
        XCTAssertEqual(made.steps[0].title, "Merge it into main")
        XCTAssertEqual(made.steps[0].command, "git merge --no-ff integration/plan-12")
        XCTAssertNil(made.steps[0].action)
        XCTAssertEqual(made.steps[1].title, "The project manager confirms the merge")
        XCTAssertEqual(made.steps[2].hostText?.shown, "Run npm ci…")
        XCTAssertNil(made.steps[2].title)
    }

    func test_Steps_theMergeStepWhenTheItemIsMergeable() {
        let step = IntegrationDetails(
            recorded: true,
            impact: IntegrationImpact(
                filesChanged: 1, insertions: 1, deletions: 1, nextSteps: [IntegrationNextStep(text: text("Run npm ci"))]))
        let made = rows(item(details: step), merge: true)
        XCTAssertEqual(made.steps.count, 2, "Merge, then the host step; no confirm step is left")
        XCTAssertEqual(made.steps[0].action, .merge)
        XCTAssertNil(made.steps[0].command)
        XCTAssertEqual(made.steps[1].hostText?.shown, "Run npm ci")
    }

    func test_Steps_hostStepsOnlyFromRecordedDetails() {
        XCTAssertEqual(rows(item(details: nil)).steps.count, 2)
        XCTAssertEqual(rows(item(details: IntegrationDetails(recorded: false))).steps.count, 2)
    }

    func test_Steps_aBranchIsQuotedOrLeftOutOfTheCommand() {
        let odd = rows(item(branch: text("x; rm -rf ~"), details: nil))
        XCTAssertEqual(odd.steps[0].command, "git merge --no-ff 'x; rm -rf ~'")
        XCTAssertNil(rows(item(branch: text("secret", verified: false), details: nil)).steps[0].command)
        XCTAssertNil(rows(item(branch: text("-x"), details: nil)).steps[0].command)
        XCTAssertNil(rows(item(branch: text("a\u{202E}b"), details: nil)).steps[0].command)
    }

    func test_Steps_unknownStateOffersNoMerge() {
        XCTAssertTrue(rows(item(state: "weird", details: nil)).steps.isEmpty)
    }

    func test_Title_unverifiedBranchIsNotShown() {
        let made = rows(item(branch: text("secret", verified: false), details: nil))
        XCTAssertEqual(made.title, "The integration is built, not in main yet")
    }

    // MARK: text

    func test_Grouped() {
        XCTAssertEqual(IntegrationDetailRows.grouped(0), "0")
        XCTAssertEqual(IntegrationDetailRows.grouped(999), "999")
        XCTAssertEqual(IntegrationDetailRows.grouped(1000), "1,000")
        XCTAssertEqual(IntegrationDetailRows.grouped(1234567), "1,234,567")
    }

    func test_Ago_todayIsRelativeAndEarlierDaysAreAClock() {
        XCTAssertEqual(IntegrationDetailRows.ago("2026-10-29T13:59:30Z", now: now, calendar: utc), "just now")
        XCTAssertEqual(IntegrationDetailRows.ago("2026-10-29T12:00:00Z", now: now, calendar: utc), "2 h ago")
        XCTAssertEqual(IntegrationDetailRows.ago("2026-10-28T23:50:00Z", now: now, calendar: utc), "yesterday 23:50")
        XCTAssertEqual(IntegrationDetailRows.ago("2026-10-03T08:05:00Z", now: now, calendar: utc), "Oct 3")
        XCTAssertEqual(IntegrationDetailRows.ago("garbage", now: now, calendar: utc), "garbage")
    }
}
