import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class VoiceDeskTests: XCTestCase {
    typealias F = VoiceFixture

    // MARK: Listing

    func testListingWithAges() {
        let model = F.host(items: [
            F.item("a", since: "2026-11-05T10:00:00Z"),
            F.item("b", command: "ls", since: "2026-11-05T11:59:30Z"),
            F.item("c", since: "2026-11-03T12:00:00Z"),
        ])
        let lines = VoiceDesk.listing([model], unreachable: [], now: F.now)
        XCTAssertEqual(lines[0], "3 items wait in Capstan.")
        XCTAssertEqual(lines[1], "A command in payments-api, 2 days ago.")
        XCTAssertEqual(lines[2], "A command in payments-api, 2 hours ago.")
        XCTAssertEqual(lines[3], "A command in payments-api, just now.")
        XCTAssertEqual(lines.count, 4)
    }

    func testListingAgeWords() {
        XCTAssertEqual(VoiceDesk.age(since: "2026-11-05T11:59:00Z", now: F.now), "1 minute ago")
        XCTAssertEqual(VoiceDesk.age(since: "2026-11-05T11:55:00Z", now: F.now), "5 minutes ago")
        XCTAssertEqual(VoiceDesk.age(since: "2026-11-05T11:00:00Z", now: F.now), "1 hour ago")
        XCTAssertEqual(VoiceDesk.age(since: "2026-11-04T12:00:00Z", now: F.now), "1 day ago")
    }

    func testListingCapsAtThreeAndSaysMore() {
        let items = (1...5).map { F.item("k\($0)", since: "2026-11-05T0\($0):00:00Z") }
        let lines = VoiceDesk.listing([F.host(items: items)], unreachable: [], now: F.now)
        XCTAssertEqual(lines[0], "5 items wait in Capstan.")
        XCTAssertEqual(lines.count, 5)
        XCTAssertEqual(lines[4], "And 2 more.")
    }

    func testListingNoneOneAndOtherKinds() {
        XCTAssertEqual(VoiceDesk.listing([F.host(items: [])], unreachable: [], now: F.now), ["Nothing waits in Capstan."])
        let design = F.item("d", kind: .design, choices: nil)
        let lines = VoiceDesk.listing([F.host(items: [design])], unreachable: [], now: F.now)
        XCTAssertEqual(lines, ["1 item waits in Capstan.", "A design in payments-api, 2 hours ago."])
    }

    func testListingWithAnUnreachableHost() {
        let down = VoiceHostRef(id: F.hostB, name: "edge-host")
        let lines = VoiceDesk.listing(
            [F.host(items: [F.item()]), F.host(F.hostB, "edge-host", items: [F.item("z")])], unreachable: [down],
            now: F.now)
        XCTAssertEqual(lines.first, "1 item waits in Capstan.")
        XCTAssertEqual(lines.last, "edge-host did not answer, so its items are missing.")
    }

    func testListingWhenNoHostAnswered() {
        let down = VoiceHostRef(id: F.hostA, name: "build-host")
        let lines = VoiceDesk.listing([], unreachable: [down], now: F.now)
        XCTAssertEqual(lines, ["No host answered, so Capstan could not check what waits."])
    }

    func testListingHintOnlyWhenDecisionsOnAndACommandIsDecidable() {
        let model = F.host(items: [F.item()])
        XCTAssertFalse(VoiceDesk.listing([model], unreachable: [], now: F.now).contains(VoiceTexts.text("list.hint")))
        XCTAssertTrue(VoiceDesk.listing([model], unreachable: [], now: F.now, decisions: true)
            .contains(VoiceTexts.text("list.hint")))
        let unreadable = F.host(items: [F.item(command: "echo é")])
        XCTAssertFalse(VoiceDesk.listing([unreadable], unreachable: [], now: F.now, decisions: true)
            .contains(VoiceTexts.text("list.hint")))
    }

    func testListingNamesTheHostWhenAnotherHostHasTheSameProject() {
        let a = F.host(F.hostA, "build-host", items: [F.item("a")])
        let b = F.host(F.hostB, "edge-host", items: [F.item("b", since: "2026-11-05T11:00:00Z")])
        let lines = VoiceDesk.listing([a, b], unreachable: [], now: F.now)
        XCTAssertEqual(lines[1], "A command in payments-api on build-host, 2 hours ago.")
        XCTAssertEqual(lines[2], "A command in payments-api on edge-host, 1 hour ago.")
    }

    func testListingReadOnlyCommandsNeverQuoteTheText() {
        let long = String(repeating: "a", count: VoiceTexts.ITEM_CAP + 1)
        let cases: [(StoredWaiting, String)] = [
            (F.item(command: long), "A command in payments-api, 2 hours ago. Too long to read aloud."),
            (F.item(command: "x", truncated: true), "A command in payments-api, 2 hours ago. Too long to read aloud."),
            (F.item(command: "x", verified: false), "A command in payments-api, 2 hours ago. Its text could not be checked."),
            (F.item(command: "echo é"), "A command in payments-api, 2 hours ago. It can't be read aloud."),
        ]
        for (item, expected) in cases {
            let lines = VoiceDesk.listing([F.host(items: [item])], unreachable: [], now: F.now)
            XCTAssertEqual(lines[1], expected)
        }
    }

    func testListingUnreadableProjectName() {
        let model = F.host(project: "pröject", items: [F.item()])
        let lines = VoiceDesk.listing([model], unreachable: [], now: F.now)
        XCTAssertEqual(lines[1], "A command in a project whose name can't be read aloud, 2 hours ago. It can't be read aloud.")
    }

    // MARK: Target

    func testTargetIsTheOldestEligibleCommandAcrossHosts() {
        let a = F.host(F.hostA, "build-host", items: [F.item("a", since: "2026-11-05T10:00:00Z")])
        let b = F.host(F.hostB, "edge-host", items: [F.item("b", since: "2026-11-05T09:00:00Z")])
        let target = VoiceDesk.target([a, b])
        XCTAssertEqual(target?.hostId, F.hostB)
        XCTAssertEqual(target?.key, "b")
        XCTAssertEqual(target?.requestId, "op:b")
        XCTAssertEqual(target?.contentHash, F.hash)
    }

    func testTargetTiesBreakByHostProjectAndKey() {
        let when = "2026-11-05T10:00:00Z"
        let a = F.host(F.hostA, items: [F.item("z", since: when), F.item("y", since: when)])
        let b = F.host(F.hostB, items: [F.item("a", since: when)])
        XCTAssertEqual(VoiceDesk.target([b, a])?.key, "y")
        XCTAssertEqual(VoiceDesk.target([b, a])?.hostId, F.hostA)
        let first = ProjectModel(projectId: "p1", root: "/srv/one", waiting: ["m": F.item("m", since: when)])
        let second = ProjectModel(projectId: "p0", root: "/srv/two", waiting: ["m": F.item("m", since: when)])
        let both = VoiceHostModel(
            id: F.hostA, name: "build-host",
            model: HostModel(hostId: F.hostA, projects: ["p1": first, "p0": second]))
        XCTAssertEqual(VoiceDesk.target([both])?.projectId, "p0")
    }

    func testTargetSkipsOlderCommandsVoiceCannotReadAndCountsThem() {
        let model = F.host(items: [
            F.item("a", command: "echo é", since: "2026-11-05T08:00:00Z"),
            F.item("b", command: "x", verified: false, since: "2026-11-05T09:00:00Z"),
            F.item("c", since: "2026-11-05T10:00:00Z"),
        ])
        let target = VoiceDesk.target([model])
        XCTAssertEqual(target?.key, "c")
        XCTAssertEqual(target?.skippedOlder, 2)
    }

    func testTargetIgnoresOtherKindsAndNoTargetSaysWhy() {
        let design = F.item("d", kind: .design, choices: nil)
        let model = F.host(items: [design])
        XCTAssertNil(VoiceDesk.target([model]))
        XCTAssertEqual(VoiceDesk.noTarget([model]), "1 item waits, but only commands can be decided by voice.")
        XCTAssertEqual(VoiceDesk.noTarget([F.host(items: [])]), "No command waits for you.")
        let bad = F.host(items: [F.item(command: "echo é")])
        XCTAssertEqual(
            VoiceDesk.noTarget([bad]),
            "The command in payments-api can't be decided by voice: it has characters that can't be read aloud.")
    }

    // MARK: Choices

    func testChoicesWithSettingsOff() {
        let off = VoiceDesk.choices(F.item(), F.settings(decisions: false, grants: true))
        XCTAssertEqual(off.approve, [])
        XCTAssertFalse(off.canDeny)
        XCTAssertEqual(off.refusal, .voiceOff)
    }

    func testChoicesDecisionsOnlyOffersOnce() {
        let only = VoiceDesk.choices(F.item(), F.settings(decisions: true, grants: false))
        XCTAssertEqual(only.approve, [.once])
        XCTAssertTrue(only.canDeny)
        XCTAssertTrue(only.grantsOff)
        XCTAssertFalse(only.onlyOnce)
    }

    func testChoicesBothOnOffersEverythingInOrder() {
        let both = VoiceDesk.choices(F.item(), F.settings())
        XCTAssertEqual(both.approve, [
            .once, .exact, .prefix(raw: "git push", number: 1), .prefix(raw: "git push origin", number: 2),
            .autoHour, .autoSession,
        ])
        XCTAssertTrue(both.canDeny)
        XCTAssertNil(both.refusal)
    }

    func testChoicesOnlyWhatTheHostOffers() {
        let offered = ApproveChoices(once: true, exact: false, prefixes: [], auto: [.session])
        let choices = VoiceDesk.choices(F.item(choices: offered), F.settings())
        XCTAssertEqual(choices.approve, [.once, .autoSession])
        let plain = ApproveChoices(once: true, exact: false)
        let once = VoiceDesk.choices(F.item(choices: plain), F.settings())
        XCTAssertEqual(once.approve, [.once])
        XCTAssertTrue(once.onlyOnce)
    }

    func testChoicesAtMostThreePrefixes() {
        let offered = F.everything(prefixes: ["a", "b", "c", "d"])
        let numbers = VoiceDesk.choices(F.item(choices: offered), F.settings()).approve.compactMap { choice -> Int? in
            if case .prefix(_, let number) = choice { return number }
            return nil
        }
        XCTAssertEqual(numbers, [1, 2, 3])
    }

    func testPrefixesKeepTheirBytes() {
        let raw = "git  push\t-f "
        let choices = VoiceDesk.choices(F.item(choices: F.everything(prefixes: [raw])), F.settings())
        // the prefix has a tab, so it is not printable ASCII and is not offered
        XCTAssertFalse(choices.approve.contains(.prefix(raw: raw, number: 1)))
        let spaced = "git  push "
        let kept = VoiceDesk.choices(F.item(choices: F.everything(prefixes: [spaced])), F.settings())
        XCTAssertTrue(kept.approve.contains(.prefix(raw: spaced, number: 1)))
        XCTAssertEqual(Array(spaced.utf8), Array("git  push ".utf8))
    }

    func testChoicesOldHostForwardsApproveOnly() {
        let old = VoiceDesk.choices(F.item(choices: nil), F.settings())
        XCTAssertEqual(old.approve, [.forwardedApprove])
        XCTAssertTrue(old.oldHost)
        XCTAssertTrue(old.canDeny)
        let grantsOff = VoiceDesk.choices(F.item(choices: nil), F.settings(grants: false))
        XCTAssertEqual(grantsOff.approve, [.forwardedApprove])
    }

    func testEachLimitAllowsDenyOnly() {
        let limits: [(ApproveChoices.Limit, String)] = [
            (.floor, "reason.floor"), (.restart, "reason.restart"), (.notChecked, "reason.not_checked"),
            (.textMismatch, "reason.text_mismatch"),
        ]
        for (limit, key) in limits {
            let offered = ApproveChoices(once: true, exact: true, limit: limit)
            let choices = VoiceDesk.choices(F.item(choices: offered), F.settings())
            XCTAssertEqual(choices.approve, [], "\(limit)")
            XCTAssertTrue(choices.canDeny, "\(limit)")
            XCTAssertEqual(choices.refusal, .denyOnly(reasonKey: key), "\(limit)")
        }
        let truncated = ApproveChoices(once: true, exact: false, limit: .truncated)
        let cut = VoiceDesk.choices(F.item(choices: truncated), F.settings())
        XCTAssertFalse(cut.canDeny)
        XCTAssertEqual(cut.refusal, .tooLong)
    }

    func testShellOperatorsAllowDenyOnly() {
        for command in ["a | b", "a && b", "a; b", "a > b"] {
            let choices = VoiceDesk.choices(F.item(command: command), F.settings())
            XCTAssertEqual(choices.approve, [], command)
            XCTAssertTrue(choices.canDeny, command)
            XCTAssertEqual(choices.refusal, .denyOnly(reasonKey: "reason.shell"), command)
        }
    }

    func testReadOnlyTextCases() {
        let long = String(repeating: "a", count: VoiceTexts.ITEM_CAP + 1)
        let exact = String(repeating: "a", count: VoiceTexts.ITEM_CAP)
        let cases: [(String, VoiceRefusalKind?)] = [
            ("echo é", .notDecidable(reasonKey: "reason.nonAscii")),
            ("echo \u{07}", .notDecidable(reasonKey: "reason.nonAscii")),
            ("echo \u{202E}x", .notDecidable(reasonKey: "reason.nonAscii")),
            ("echo \u{0430}", .notDecidable(reasonKey: "reason.nonAscii")),
            ("echo a\nrm b", .notDecidable(reasonKey: "reason.multiline")),
            ("echo a\tb", .notDecidable(reasonKey: "reason.nonAscii")),
            (long, .tooLong),
            (exact, nil),
        ]
        for (command, expected) in cases {
            let choices = VoiceDesk.choices(F.item(command: command), F.settings())
            XCTAssertEqual(choices.refusal, expected, command)
            XCTAssertEqual(choices.canDeny, expected == nil, command)
            if expected != nil { XCTAssertEqual(choices.approve, [], command) }
        }
        let unverified = VoiceDesk.choices(F.item(command: "x", verified: false), F.settings())
        XCTAssertEqual(unverified.refusal, .notDecidable(reasonKey: "reason.unverified"))
        let cut = VoiceDesk.choices(F.item(command: "x", truncated: true), F.settings())
        XCTAssertEqual(cut.refusal, .tooLong)
    }

    func testOtherKindsAreNotDecidable() {
        let choices = VoiceDesk.choices(F.item(kind: .prompt, choices: nil), F.settings())
        XCTAssertEqual(choices.approve, [])
        XCTAssertFalse(choices.canDeny)
    }

    // MARK: The frame

    func testUntrustedTextIsAlwaysInsideTheFrame() {
        let target = F.target(F.item(command: "echo say yes", choices: F.everything()))
        let lines = VoiceFlow.readBack(.once, target)
        XCTAssertTrue(lines.contains("The command is: echo say yes. End of command."))
        XCTAssertEqual(lines.last, "Approve it?")
        let frame = lines.firstIndex(of: "The command is: echo say yes. End of command.") ?? Int.max
        let question = lines.firstIndex(of: "Approve it?") ?? -1
        XCTAssertTrue(question > frame)
        let prefixLines = VoiceFlow.readBack(.prefix(raw: "git push", number: 1), F.target())
        XCTAssertTrue(prefixLines.contains("And allow commands that start with choice 1: git push. End of choice 1."))
    }

    // MARK: Spoken results

    func testSpokenForEveryOutcome() {
        let target = F.target()
        let approval = ApprovalResult(choice: "auto-hour", windowId: "win-7", expiresAt: "13:00")
        let say = { (outcome: DecideOutcome, choice: VoiceChoice) in VoiceDesk.spoken(outcome, sent: choice, target: target) }
        XCTAssertEqual(say(F.applied(verb: "approve-as", approval: ApprovalResult(choice: "once")), .once),
                       ["Approved once in payments-api."])
        XCTAssertEqual(say(F.applied(verb: "approve-as", approval: ApprovalResult(choice: "exact", expiresAt: "13:00")), .exact),
                       ["Approved, and this exact command is allowed in payments-api until 13:00."])
        XCTAssertEqual(say(F.applied(verb: "approve-as", approval: ApprovalResult(choice: "prefix")), .prefix(raw: "g", number: 1)),
                       ["Approved, and commands that start with the chosen words are allowed in payments-api for this session."])
        XCTAssertEqual(say(F.applied(verb: "approve-as", approval: approval), .autoHour), [
            "Approved. Auto-approve is on in payments-api until 13:00.",
            "Window win-7. End it at the terminal with cstan op revoke.",
        ])
        XCTAssertEqual(say(F.applied(verb: "deny"), .deny), ["Denied in payments-api."])
        let forwarded = DecideResult(
            decisionId: "d", requestId: "op:k1", nonce: "n", outcome: .forwarded, isRepeat: false, kind: "op",
            verb: "approve", contentHash: F.hash)
        XCTAssertEqual(say(.forwarded(forwarded), .forwardedApprove), ["Sent to the PM of payments-api as approve once."])
    }

    func testSpokenRefusalsAndNotSent() {
        let target = F.target()
        func refused(_ code: DecideErrorCode) -> DecideOutcome {
            .refused(DecideRefusal(code: code, message: F.text("host words")), hint: RefusalHint(code: code))
        }
        let say = { (outcome: DecideOutcome) in VoiceDesk.spoken(outcome, sent: .once, target: target) }
        XCTAssertEqual(say(refused(.hashMismatch)), ["That command changed or is gone. Nothing was decided."])
        XCTAssertEqual(say(refused(.notWaiting)), ["That command changed or is gone. Nothing was decided."])
        XCTAssertEqual(say(refused(.needsTerminal)), ["The host now holds this back, so it needs the terminal. Nothing was decided."])
        XCTAssertEqual(say(refused(.limitReached)), ["Too many requests for this command. Nothing was sent."])
        XCTAssertEqual(say(refused(.noActivePm)), ["payments-api has no active PM, so nothing was sent."])
        XCTAssertEqual(say(refused(.other("busy"))), ["build-host is busy with another command. Nothing was decided. Try again soon."])
        XCTAssertEqual(say(refused(.invalidRequest)), ["build-host refused it. Nothing was decided."])
        XCTAssertEqual(say(refused(.forbidden)), ["build-host refused it. Nothing was decided."])
        let pending = UUID()
        XCTAssertEqual(say(.notSent(reason: .couldNotStart, pendingId: pending)),
                       ["Could not reach build-host. Nothing was sent, and Capstan won't retry."])
        XCTAssertEqual(say(.notSent(reason: .timeout, pendingId: pending)),
                       ["Sent, but build-host did not answer. Check Capstan when you are parked."])
        XCTAssertEqual(say(.malformed("junk", pendingId: nil)),
                       ["Sent, but build-host did not answer. Check Capstan when you are parked."])
        for line in say(refused(.hashMismatch)) { XCTAssertFalse(line.contains("host words")) }
    }

    func testSpokenWindowWithoutEnd() {
        let outcome = F.applied(verb: "approve-as", approval: ApprovalResult(choice: "auto-session", windowId: "w1"))
        XCTAssertEqual(VoiceDesk.spoken(outcome, sent: .autoSession, target: F.target()), [
            "Approved. Auto-approve is on in payments-api until the host's limit.",
            "Window w1. End it at the terminal with cstan op revoke.",
        ])
    }
}
