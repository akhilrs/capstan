import CapstanKit
import XCTest

@testable import CapstanAppCore

final class OutcomePresentationTests: XCTestCase {
    private func present(_ outcome: DecideOutcome?, host: String = "box") -> Presentation {
        Presentation.present(outcome, hostName: host)
    }

    private func refused(_ code: DecideErrorCode, _ hint: RefusalHint) -> DecideOutcome {
        .refused(ShellFixtures.refusal(code), hint: hint)
    }

    func test_Presentation_sending() {
        XCTAssertEqual(present(nil), Presentation(title: "Sending", tone: .neutral, actions: []))
    }

    func test_Presentation_localBusy() {
        XCTAssertEqual(present(LocalOutcome.busy()), Presentation(title: "Already sending", tone: .grey, actions: []))
    }

    func test_Presentation_repeatApplied() {
        let outcome = DecideOutcome.applied(ShellFixtures.result(outcome: .applied, isRepeat: true))
        XCTAssertEqual(present(outcome), Presentation(title: "Already sent", tone: .grey, actions: []))
    }

    func test_Presentation_repeatForwarded() {
        let outcome = DecideOutcome.forwarded(ShellFixtures.result(outcome: .forwarded, isRepeat: true))
        XCTAssertEqual(present(outcome), Presentation(title: "Already sent", tone: .grey, actions: []))
    }

    func test_Presentation_applied() {
        let outcome = DecideOutcome.applied(ShellFixtures.result(outcome: .applied))
        XCTAssertEqual(present(outcome), Presentation(title: "Done", tone: .green, actions: []))
    }

    func test_Presentation_forwarded() {
        let outcome = DecideOutcome.forwarded(ShellFixtures.result(outcome: .forwarded))
        XCTAssertEqual(present(outcome), Presentation(title: "Forwarded to the PM, waiting", tone: .blue, actions: []))
    }

    func test_Presentation_hintTerminal() {
        XCTAssertEqual(
            present(refused(.needsTerminal, .terminal)),
            Presentation(title: "Answer in the terminal", tone: .yellow, actions: []))
    }

    func test_Presentation_hintRefresh() {
        XCTAssertEqual(
            present(refused(.hashMismatch, .refresh)),
            Presentation(title: "This item changed", tone: .yellow, actions: [.refresh]))
    }

    func test_Presentation_hintGone() {
        XCTAssertEqual(
            present(refused(.notWaiting, .gone)),
            Presentation(title: "No longer waiting", tone: .grey, actions: []))
    }

    func test_Presentation_hintNoPm() {
        XCTAssertEqual(
            present(refused(.noActivePm, .noPm)),
            Presentation(title: "No active PM in this project", tone: .red, actions: []))
    }

    func test_Presentation_hintLimit() {
        XCTAssertEqual(
            present(refused(.limitReached, .limit)),
            Presentation(title: "Too many forwards for this item", tone: .yellow, actions: []))
    }

    func test_Presentation_hintInternal() {
        XCTAssertEqual(
            present(refused(.nonceReused, .internal)),
            Presentation(title: "Could not send", tone: .red, actions: [.tryAgain]))
    }

    func test_Presentation_hintInvalid() {
        XCTAssertEqual(
            present(refused(.invalidRequest, .invalid)),
            Presentation(title: "The host rejected the request", tone: .red, actions: []))
    }

    func test_Presentation_hintHost() {
        XCTAssertEqual(
            present(refused(.forbidden, .host)),
            Presentation(title: "The host refused: forbidden", tone: .red, actions: []))
    }

    func test_Presentation_hintGeneric() {
        XCTAssertEqual(
            present(refused(.other("weird_code"), .generic)),
            Presentation(title: "Refused: weird_code", tone: .red, actions: []))
    }

    func test_Presentation_unknownVerbMeansOlderController() {
        let older = Presentation(
            title: "This project's Capstan is older than the app. Restart it on the host: cstan restart",
            tone: .red, actions: [])
        XCTAssertEqual(present(refused(.other("unknown_command"), .generic)), older)
        XCTAssertEqual(present(refused(.other("unsupported"), .generic)), older)
        // Any other code keeps its own wording.
        XCTAssertEqual(
            present(refused(.other("weird_code"), .generic)),
            Presentation(title: "Refused: weird_code", tone: .red, actions: []))
    }

    func test_Presentation_notSent() {
        XCTAssertEqual(
            present(.notSent(reason: .timeout, pendingId: UUID()), host: "devbox"),
            Presentation(title: "Could not reach devbox", tone: .red, actions: [.retry]))
    }

    func test_Presentation_malformed() {
        XCTAssertEqual(
            present(.malformed("x", pendingId: nil)),
            Presentation(title: "Unexpected answer from the host", tone: .red, actions: [.retry]))
    }

    func test_Presentation_hostileCodeCleanedAndCut() {
        let hostile = "\u{202E}" + String(repeating: "x", count: 10_240)
        let result = present(refused(.other(hostile), .generic))
        XCTAssertFalse(result.title.unicodeScalars.contains("\u{202E}"))
        XCTAssertTrue(result.title.hasPrefix("Refused:  x"))
        XCTAssertEqual(result.title.utf8.count, "Refused: ".utf8.count + 64)
        let host = present(.notSent(reason: .timeout, pendingId: UUID()), host: hostile)
        XCTAssertFalse(host.title.unicodeScalars.contains("\u{202E}"))
        XCTAssertEqual(host.title.utf8.count, "Could not reach ".utf8.count + 64)
    }
}
