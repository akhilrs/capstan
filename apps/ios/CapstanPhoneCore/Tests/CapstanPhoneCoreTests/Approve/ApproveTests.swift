import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class ApproveTests: XCTestCase {
    private func text(_ raw: String) -> UntrustedText { UntrustedText(raw: raw, verified: true, truncated: false) }

    private func choices(
        once: Bool = true, exact: Bool = true, prefixes: [String] = ["git ", "git status "],
        auto: [ApproveChoices.AutoKind] = [.hour, .session], limit: ApproveChoices.Limit? = nil, floorEntry: String? = nil
    ) -> ApproveChoices {
        ApproveChoices(
            once: once, exact: exact, prefixes: prefixes.map(text), auto: auto, limit: limit, floorEntry: floorEntry)
    }

    private func plan(_ c: ApproveChoices) throws -> ApprovePlan {
        guard case .sheet(let plan) = ApproveOffer.make(c, commandVerified: true, commandWhole: true) else {
            throw XCTSkip("no sheet")
        }
        return plan
    }

    // MARK: Offer

    func testOldHostKeepsTheForwardFlow() {
        XCTAssertEqual(ApproveOffer.make(nil, commandVerified: true, commandWhole: true), .oldHost)
        XCTAssertEqual(ApproveOffer.make(nil, commandVerified: false, commandWhole: false), .oldHost)
    }

    func testACutOrUnverifiedCommandOffersNoSheet() {
        XCTAssertEqual(ApproveOffer.make(choices(), commandVerified: false, commandWhole: true), .denyOnly(nil))
        XCTAssertEqual(ApproveOffer.make(choices(), commandVerified: true, commandWhole: false), .denyOnly(nil))
    }

    func testTextMismatchGivesDenyOnlyAndNoForward() throws {
        let c = choices(once: true, limit: .textMismatch)
        XCTAssertEqual(ApproveOffer.make(c, commandVerified: true, commandWhole: true), .denyOnly(.textMismatch))
        XCTAssertEqual(approveDenyOnlyText(.textMismatch), ApproveStrings.limitedTextMismatch)
    }

    func testTruncatedGivesDenyOnly() {
        let c = choices(limit: .truncated)
        XCTAssertEqual(ApproveOffer.make(c, commandVerified: true, commandWhole: true), .denyOnly(.truncated))
        XCTAssertEqual(approveDenyOnlyText(.truncated), ApproveStrings.limitedTruncated)
    }

    func testLimitedItemsOfferOnceOnly() throws {
        for limit: ApproveChoices.Limit in [.floor, .restart, .notChecked] {
            let p = try plan(choices(limit: limit, floorEntry: limit == .floor ? "rm -rf" : nil))
            XCTAssertEqual(p.tops, [.once], "\(limit)")
            XCTAssertTrue(p.scopes.isEmpty)
            XCTAssertTrue(p.windows.isEmpty)
            XCTAssertNotNil(p.limitText)
        }
        XCTAssertEqual(try plan(choices(limit: .floor, floorEntry: "rm -rf")).floorEntry, "rm -rf")
    }

    func testNothingOfferedGivesDenyOnly() {
        let c = choices(once: false, exact: false, prefixes: [], auto: [])
        XCTAssertEqual(ApproveOffer.make(c, commandVerified: true, commandWhole: true), .denyOnly(nil))
    }

    func testAnEmptyPrefixIsNotOffered() throws {
        let p = try plan(choices(exact: false, prefixes: ["", "make "]))
        XCTAssertEqual(p.scopes, [.prefix(raw: "make ")])
    }

    func testOnlyListedAutoWindowsAreOffered() throws {
        let p = try plan(choices(auto: [.session]))
        XCTAssertEqual(p.windows, [.session])
        XCTAssertFalse(try plan(choices(auto: [])).tops.contains(.auto))
    }

    // MARK: Prefix

    func testThePrefixIsNeverTrimmedSplitOrRejoined() throws {
        let odd = " git  status\t\u{00A0}"
        let p = try plan(choices(prefixes: [odd]))
        XCTAssertEqual(p.scopes, [.exact, .prefix(raw: odd)])
        var s = ApproveSheetState(plan: p)
        s.pick(.similar)
        XCTAssertTrue(s.advance())
        s.pick(.prefix(raw: odd))
        XCTAssertEqual(s.request, .opApproveAs(.prefix(raw: odd)))
        guard case .opApproveAs(let choice)? = s.request, case .prefix(let sent) = choice else {
            return XCTFail("not a prefix")
        }
        XCTAssertEqual(Array(sent.utf8), Array(odd.utf8))
    }

    func testThePrefixRowTextEqualsTheOfferedPrefix() throws {
        let offered = ["cargo ", "cargo test --workspace "]
        let p = try plan(choices(prefixes: offered))
        let rows = p.scopes.compactMap { scope -> String? in
            if case .prefix(let raw) = scope { return raw }
            return nil
        }
        XCTAssertEqual(rows, offered)
    }

    func testAPrefixThatWasNotOfferedCannotBePicked() throws {
        var s = ApproveSheetState(plan: try plan(choices(prefixes: ["git "])))
        s.pick(.similar)
        _ = s.advance()
        s.pick(.prefix(raw: "git"))
        XCTAssertNil(s.scope)
        XCTAssertNil(s.request)
    }

    // MARK: Steps and verbs

    func testOnceSendsFromStepOne() throws {
        var s = ApproveSheetState(plan: try plan(choices()))
        XCTAssertEqual(s.primaryLabel, ApproveStrings.choicesNone)
        XCTAssertNil(s.request)
        s.pick(.once)
        XCTAssertEqual(s.primaryLabel, ApproveStrings.onceConfirmAction)
        XCTAssertEqual(s.request, .opApproveAs(.once))
        XCTAssertNil(s.stepText)
    }

    func testExactGivesTheExactVerb() throws {
        var s = ApproveSheetState(plan: try plan(choices()))
        s.pick(.similar)
        XCTAssertNil(s.request)
        XCTAssertTrue(s.advance())
        XCTAssertEqual(s.step, .scope)
        XCTAssertEqual(s.stepText, "2 of 2")
        XCTAssertNil(s.request)
        s.pick(.exact)
        XCTAssertEqual(s.request, .opApproveAs(.exact))
    }

    func testAutoNeedsTheConfirmStep() throws {
        var s = ApproveSheetState(plan: try plan(choices()))
        s.pick(.auto)
        XCTAssertNil(s.request)
        XCTAssertTrue(s.advance())
        XCTAssertEqual(s.step, .window)
        XCTAssertFalse(s.primaryEnabled)
        s.pick(.hour)
        XCTAssertNil(s.request, "a window is not opened before the confirm")
        XCTAssertTrue(s.advance())
        XCTAssertEqual(s.step, .confirmAuto)
        XCTAssertEqual(s.stepText, "3 of 3")
        XCTAssertEqual(s.request, .opApproveAs(.autoHour))
        XCTAssertEqual(s.primaryLabel, ApproveStrings.autoConfirmAction)
        s.back()
        s.pick(.session)
        XCTAssertTrue(s.advance())
        XCTAssertEqual(s.request, .opApproveAs(.autoSession))
    }

    func testBackReturnsStepByStep() throws {
        var s = ApproveSheetState(plan: try plan(choices()))
        s.pick(.auto)
        _ = s.advance()
        s.pick(.hour)
        _ = s.advance()
        s.back()
        XCTAssertEqual(s.step, .window)
        s.back()
        XCTAssertEqual(s.step, .choose)
        s.back()
        XCTAssertEqual(s.step, .choose)
    }

    func testAnAnswerThatIsNotOfferedCannotBePicked() throws {
        var s = ApproveSheetState(plan: try plan(choices(limit: .floor)))
        XCTAssertEqual(s.top, .once, "a single offer is picked already")
        s.pick(.auto)
        XCTAssertEqual(s.top, .once)
        XCTAssertEqual(s.request, .opApproveAs(.once))
    }

    func testTheRequestBuildsTheExactArguments() throws {
        let verb = DecideVerb.opApproveAs(.prefix(raw: "git status "))
        let request = DecideRequest(
            hostId: UUID(), projectId: "p", requestId: "op:1", expect: String(repeating: "a", count: 12),
            nonce: Nonce.make(), client: "iPhone", verb: verb)
        let args = try request.remoteArguments()
        XCTAssertEqual(Array(args.suffix(4)), ["approve-as", "prefix", "--prefix", "git status "])
    }

    // MARK: Result lines

    func testResultOnceHasNoGrantOrWindow() {
        let lines = ApproveResultLines.make(approval: ApprovalResult(choice: "once"), chosen: .once, command: "ls")
        XCTAssertEqual(lines.headline, ApproveStrings.resultOnce)
        XCTAssertNil(lines.grantText)
        XCTAssertNil(lines.windowId)
        XCTAssertNil(lines.ends)
        XCTAssertNil(lines.revoke)
        XCTAssertEqual(lines.pm, ApproveStrings.resultPm)
    }

    func testResultGrantOnly() {
        let lines = ApproveResultLines.make(
            approval: ApprovalResult(choice: "prefix", grantId: "g1"), chosen: .prefix(raw: "git "), command: "git status")
        XCTAssertEqual(lines.headline, ApproveStrings.resultGrant)
        XCTAssertEqual(lines.grantText, "git ")
        XCTAssertNil(lines.windowId)
        XCTAssertNil(lines.ends)
        let exact = ApproveResultLines.make(
            approval: ApprovalResult(choice: "exact", grantId: "g2"), chosen: .exact, command: "git status")
        XCTAssertEqual(exact.grantText, "git status")
    }

    func testResultWindowOnly() {
        let lines = ApproveResultLines.make(
            approval: ApprovalResult(choice: "auto-hour", windowId: "w7", expiresAt: "2026-11-06T14:05:00+05:30"),
            chosen: .autoHour, command: "ls")
        XCTAssertEqual(lines.headline, ApproveStrings.resultWindow)
        XCTAssertEqual(lines.windowId, "Window w7")
        XCTAssertEqual(lines.ends, "Ends 14:05")
        XCTAssertEqual(lines.revoke, ApproveStrings.resultWindowRevoke)
        XCTAssertNil(lines.grantText)
    }

    func testAWindowWithoutAnExpiryClaimsNoEnd() {
        let lines = ApproveResultLines.make(
            approval: ApprovalResult(choice: "auto-session", windowId: "w8"), chosen: .autoSession, command: "ls")
        XCTAssertEqual(lines.windowId, "Window w8")
        XCTAssertNil(lines.ends)
    }

    func testAResultWithoutApprovalReadsAsOnce() {
        let lines = ApproveResultLines.make(approval: nil, chosen: .once, command: "ls")
        XCTAssertEqual(lines.headline, ApproveStrings.resultOnce)
    }

    // MARK: Refusals

    private func refused(_ code: DecideErrorCode, _ message: String = "") -> DecideOutcome {
        .refused(DecideRefusal(code: code, message: text(message)), hint: RefusalHint(code: code))
    }

    func testRefusalLines() {
        XCTAssertEqual(ApproveRefusalLines.make(refused(.hashMismatch))?.action, .showNew)
        XCTAssertEqual(ApproveRefusalLines.make(refused(.hashMismatch))?.text, ApproveStrings.refusalHashMismatch)
        XCTAssertEqual(ApproveRefusalLines.make(refused(.notWaiting))?.replacesActions, true)
        XCTAssertEqual(ApproveRefusalLines.make(refused(.needsTerminal))?.text, ApproveStrings.refusalNeedsTerminal)
        XCTAssertEqual(ApproveRefusalLines.make(refused(.other("local-busy")))?.text, ApproveStrings.refusalBusy)
        let invalid = ApproveRefusalLines.make(refused(.invalidRequest, "bad prefix"))
        XCTAssertEqual(invalid?.text, ApproveStrings.refusalInvalidRequest)
        XCTAssertEqual(invalid?.hostText?.raw, "bad prefix")
        XCTAssertNil(ApproveRefusalLines.make(refused(.forbidden)))
        XCTAssertNil(ApproveRefusalLines.make(.notSent(reason: .timeout, pendingId: UUID())))
    }

    // MARK: Strings

    func testStringsEqualTheDesign() throws {
        var directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        var found: URL?
        while directory.path != "/" {
            let candidate = directory.appendingPathComponent("docs/design/plan-153-design-approve/strings.json")
            if FileManager.default.fileExists(atPath: candidate.path) { found = candidate; break }
            directory = directory.deletingLastPathComponent()
        }
        guard let path = found, let data = try? Data(contentsOf: path),
            let design = try JSONSerialization.jsonObject(with: data) as? [String: String]
        else { throw XCTSkip("strings.json not found above \(#filePath)") }
        XCTAssertEqual(ApproveStrings.opTitle, design["op.title"])
        XCTAssertEqual(ApproveStrings.resultPm, design["result.pm"])
        XCTAssertEqual(ApproveStrings.autoCoverage, design["auto.coverage"])
        XCTAssertEqual(ApproveStrings.oldhostNote, design["oldhost.note"])
    }
}
