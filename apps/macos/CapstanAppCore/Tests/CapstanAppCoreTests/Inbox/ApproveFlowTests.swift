import CapstanKit
import XCTest

@testable import CapstanAppCore

final class ApproveFlowTests: XCTestCase {
    private func text(_ s: String) -> UntrustedText { UntrustedText(raw: s, verified: true, truncated: false) }

    private func choices(
        once: Bool = true, exact: Bool = true, prefixes: [String] = ["git commit", "git commit -m"],
        auto: [ApproveChoices.AutoKind] = [.hour, .session], limit: ApproveChoices.Limit? = nil, floorEntry: String? = nil
    ) -> ApproveChoices {
        ApproveChoices(
            once: once, exact: exact, prefixes: prefixes.map(text), auto: auto, limit: limit, floorEntry: floorEntry)
    }

    private func flow(_ c: ApproveChoices) throws -> ApproveFlow {
        try XCTUnwrap(ApproveFlow(choices: c, commandVerified: true))
    }

    // MARK: Verbs

    func test_once_sendsApproveAsOnce() throws {
        var f = try flow(choices())
        XCTAssertFalse(f.canAdvance)
        XCTAssertEqual(f.advance(), .disabled)
        f.choose(.once)
        XCTAssertEqual(f.primaryTitle, ApproveStrings.onceConfirmAction)
        XCTAssertNil(f.stepText)
        XCTAssertEqual(f.advance(), .send(.opApproveAs(.once)))
    }

    func test_similar_exactAndPrefix() throws {
        var f = try flow(choices())
        f.choose(.similar)
        XCTAssertEqual(f.stepText, "1 of 2")
        XCTAssertEqual(f.advance(), .moved)
        XCTAssertEqual(f.step, .scope)
        XCTAssertEqual(f.stepText, "2 of 2")
        XCTAssertEqual(f.primaryTitle, ApproveStrings.scopeAction)
        XCTAssertEqual(f.advance(), .disabled)
        f.choose(scope: .exact)
        XCTAssertEqual(f.advance(), .send(.opApproveAs(.exact)))
        f.choose(scope: .prefix("git commit -m"))
        XCTAssertEqual(f.advance(), .send(.opApproveAs(.prefix(raw: "git commit -m"))))
    }

    func test_prefix_mustBeOneOfTheOffered_byteForByte() throws {
        var f = try flow(choices(prefixes: ["git commit"]))
        f.choose(.similar)
        _ = f.advance()
        f.choose(scope: .prefix("git commit "))
        XCTAssertNil(f.scope)
        f.choose(scope: .prefix("git"))
        XCTAssertNil(f.scope)
        f.choose(scope: .prefix("git commit"))
        XCTAssertEqual(f.scope, .prefix("git commit"))
    }

    func test_longPrefix_keptWhole() throws {
        let long = "cargo test -p " + String(repeating: "capstan-kernel-", count: 40) + "--"
        var f = try flow(choices(prefixes: [long]))
        f.choose(.similar)
        _ = f.advance()
        f.choose(scope: .prefix(long))
        XCTAssertEqual(f.advance(), .send(.opApproveAs(.prefix(raw: long))))
        XCTAssertEqual(f.prefixes, [long])
    }

    func test_emptyPrefix_isHidden() throws {
        let f = try flow(choices(exact: false, prefixes: ["", "git"]))
        XCTAssertEqual(f.prefixes, ["git"])
        let none = try flow(choices(exact: false, prefixes: [""]))
        XCTAssertEqual(none.answers, [.once, .auto])
    }

    func test_auto_asksBeforeTheWindow() throws {
        var f = try flow(choices())
        f.choose(.auto)
        XCTAssertEqual(f.advance(), .moved)
        XCTAssertEqual(f.step, .window)
        XCTAssertEqual(f.advance(), .disabled)
        XCTAssertNil(f.confirmedWindow())
        f.choose(window: .hour)
        XCTAssertEqual(f.primaryTitle, ApproveStrings.autoNext)
        XCTAssertEqual(f.advance(), .confirmAuto)
        XCTAssertEqual(f.confirmedWindow(), .opApproveAs(.autoHour))
        f.choose(window: .session)
        XCTAssertEqual(f.confirmedWindow(), .opApproveAs(.autoSession))
    }

    func test_back_clearsStepTwo() throws {
        var f = try flow(choices())
        f.choose(.similar)
        _ = f.advance()
        f.choose(scope: .exact)
        f.back()
        XCTAssertEqual(f.step, .answer)
        XCTAssertNil(f.scope)
        XCTAssertEqual(f.answer, .similar)
    }

    // MARK: What is offered

    func test_answers_followTheHost() throws {
        XCTAssertEqual(try flow(choices()).answers, [.once, .similar, .auto])
        XCTAssertEqual(try flow(choices(exact: false, prefixes: [])).answers, [.once, .auto])
        XCTAssertEqual(try flow(choices(auto: [])).answers, [.once, .similar])
        XCTAssertEqual(try flow(choices(auto: [.session])).windows, [.session])
        var f = try flow(choices(auto: []))
        f.choose(.auto)
        XCTAssertNil(f.answer)
    }

    func test_limits_offerOnceOnly() throws {
        for limit in [ApproveChoices.Limit.floor, .restart, .notChecked] {
            let f = try flow(choices(limit: limit, floorEntry: "sudo"))
            XCTAssertEqual(f.answers, [.once], "\(limit)")
            XCTAssertEqual(ApproveLimit.make(f.choices)?.lead, ApproveStrings.limitedLead)
        }
        XCTAssertEqual(ApproveLimit.make(choices(limit: .floor, floorEntry: "sudo"))?.floorEntry, "sudo")
        XCTAssertEqual(ApproveLimit.make(choices(limit: .floor))?.reason, ApproveStrings.limitedFloor)
        XCTAssertEqual(ApproveLimit.make(choices(limit: .restart))?.reason, ApproveStrings.limitedRestart)
        XCTAssertEqual(ApproveLimit.make(choices(limit: .notChecked))?.reason, ApproveStrings.limitedNotChecked)
        XCTAssertNil(ApproveLimit.make(choices()))
    }

    func test_truncatedAndMismatch_haveNoSheet() {
        XCTAssertNil(ApproveFlow(choices: choices(limit: .truncated), commandVerified: true))
        XCTAssertNil(ApproveFlow(choices: choices(limit: .textMismatch), commandVerified: true))
        XCTAssertNil(ApproveFlow(choices: choices(), commandVerified: false))
        XCTAssertNil(ApproveFlow(choices: choices(once: false), commandVerified: true))
        let truncated = ApproveLimit.make(choices(limit: .truncated))
        XCTAssertNil(truncated?.lead)
        XCTAssertEqual(truncated?.reason, ApproveStrings.limitedTruncated)
        XCTAssertNil(ApproveLimit.make(choices(limit: .textMismatch))?.lead)
    }

    func test_offer_forwardWithoutChoices_sheetWithChoices() throws {
        var item = try ShellFixtures.waiting(key: "op:1")
        let command = text("ls")
        XCTAssertEqual(ApproveOffer.make(item, command: command), .forward)
        item.approveChoices = choices()
        guard case .sheet = ApproveOffer.make(item, command: command) else { return XCTFail("sheet") }
        XCTAssertEqual(ApproveOffer.make(item, command: nil), .denyOnly(nil))
        let cut = UntrustedText(raw: "ls", verified: true, truncated: true)
        XCTAssertEqual(
            ApproveOffer.make(item, command: cut),
            .denyOnly(ApproveLimit(lead: nil, reason: ApproveStrings.limitedTruncated, floorEntry: nil)))
        item.approveChoices = choices(limit: .textMismatch)
        XCTAssertEqual(ApproveOffer.make(item, command: command), .denyOnly(ApproveLimit.make(item.approveChoices!)))
    }

    // MARK: Results

    private func present(_ approval: ApprovalResult?, sent: ApproveChoice) -> ApprovePresentation {
        let result = DecideResult(
            decisionId: "d", requestId: "r", nonce: "n", outcome: .applied, isRepeat: false, kind: "op",
            verb: "approve-as", contentHash: "h", approval: approval)
        return ApprovePresentation.present(.applied(result), sent: sent, hostName: "box")
    }

    private func applied(_ p: ApprovePresentation) throws -> ApprovePresentation.Applied {
        guard case .applied(let a) = p else { throw XCTSkip("not applied: \(p)") }
        return a
    }

    func test_result_once_hasNoGrantAndNoWindow() throws {
        let a = try applied(present(ApprovalResult(choice: "once"), sent: .once))
        XCTAssertEqual(a.lead, ApproveStrings.resultOnce)
        XCTAssertNil(a.prefix)
        XCTAssertNil(a.windowLine)
        XCTAssertNil(a.endsLine)
        XCTAssertEqual(a.pmLine, ApproveStrings.resultPm)
    }

    func test_result_grantOnly_endsFromTheHost() throws {
        let a = try applied(
            present(
                ApprovalResult(choice: "prefix", grantId: "g1", expiresAt: "2026-11-05T16:58:00+05:30"),
                sent: .prefix(raw: "git commit")))
        XCTAssertEqual(a.lead, ApproveStrings.resultGrant)
        XCTAssertEqual(a.prefix, "git commit")
        XCTAssertEqual(a.endsLine, "Ends 16:58 +05:30")
        XCTAssertNil(a.windowLine)
        let exact = try applied(present(ApprovalResult(choice: "exact", grantId: "g2"), sent: .exact))
        XCTAssertEqual(exact.scopeText, ApproveStrings.scopeExactTitle)
        XCTAssertNil(exact.endsLine, "a null expiresAt hides the Ends line")
    }

    func test_result_windowOnly() throws {
        let a = try applied(
            present(
                ApprovalResult(choice: "auto-hour", windowId: "win-7", expiresAt: "2026-11-05T14:41:09Z"), sent: .autoHour))
        XCTAssertEqual(a.lead, ApproveStrings.resultWindow)
        XCTAssertEqual(a.windowLine, "Window win-7")
        XCTAssertEqual(a.endsLine, "Ends 14:41 UTC")
        XCTAssertEqual(a.revokeLine, ApproveStrings.resultWindowRevoke)
        XCTAssertNil(a.prefix)
        let noTime = try applied(present(ApprovalResult(choice: "auto-session", windowId: "win-8"), sent: .autoSession))
        XCTAssertNil(noTime.endsLine)
    }

    func test_result_oldHost_withoutApproval_isThePlainDone() {
        XCTAssertEqual(
            present(nil, sent: .once),
            .other(Presentation(title: "Done", tone: .green)))
        XCTAssertEqual(ApprovePresentation.present(nil, sent: .once, hostName: "box"), .sending)
    }

    func test_prefixes_repeatedOneIsShownOnce() throws {
        let f = try flow(choices(exact: false, prefixes: ["git", "git", "git commit"]))
        XCTAssertEqual(f.prefixes, ["git", "git commit"])
    }

    func test_result_clockWithoutT_isShownAsSent() {
        XCTAssertEqual(ApprovePresentation.hostClock("soon"), "soon")
    }

    func test_refusals() throws {
        func refused(_ code: DecideErrorCode) -> ApprovePresentation.Refused? {
            let outcome = DecideOutcome.refused(ShellFixtures.refusal(code), hint: .generic)
            if case .refused(let r) = ApprovePresentation.present(outcome, sent: .once, hostName: "box") { return r }
            return nil
        }
        XCTAssertEqual(refused(.other("busy"))?.tone, .yellow)
        XCTAssertEqual(refused(.other("busy"))?.primaryLive, true)
        XCTAssertEqual(refused(.notWaiting)?.closesSheet, true)
        let hash = refused(.hashMismatch)
        XCTAssertEqual(hash?.action, ApproveStrings.refusalHashMismatchAction)
        XCTAssertEqual(hash?.primaryLive, false)
        XCTAssertEqual(refused(.needsTerminal)?.showsHostMessage, true)
        XCTAssertNotNil(refused(.needsTerminal)?.hostMessage)
        XCTAssertNil(refused(.hashMismatch)?.hostMessage)
        XCTAssertEqual(refused(.invalidRequest)?.text, ApproveStrings.refusalInvalidRequest)
        XCTAssertEqual(refused(.invalidRequest)?.nothing, ApproveStrings.refusalNothing)
        XCTAssertNil(refused(.forbidden))
        if case .refused = ApprovePresentation.present(LocalOutcome.busy(), sent: .once, hostName: "box") {} else {
            XCTFail("local busy")
        }
    }
}
