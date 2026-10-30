import XCTest
@testable import CapstanKit

final class ArgvTests: XCTestCase {
    private let head = ["decide", "--project", "proj-1", "--json"]
    private func tail(_ id: String) -> [String] {
        [id, "--expect", testHash, "--nonce", testNonce, "--client", "Mac"]
    }

    private func args(_ verb: DecideVerb, id: String = "op:p1") throws -> [String] {
        try makeRequest(verb: verb, requestId: id).remoteArguments()
    }

    func testExactArgsForEveryVerb() throws {
        XCTAssertEqual(try args(.opDeny(note: nil)), head + tail("op:p1") + ["deny"])
        XCTAssertEqual(try args(.opDeny(note: "too risky")), head + tail("op:p1") + ["deny", "too risky"])
        XCTAssertEqual(try args(.opApprove), head + tail("op:p1") + ["approve"])
        XCTAssertEqual(try args(.promptOption(1), id: "prompt:r"), head + tail("prompt:r") + ["option", "1"])
        XCTAssertEqual(try args(.promptEsc, id: "prompt:r"), head + tail("prompt:r") + ["esc"])
        XCTAssertEqual(
            try args(.designApprove(link: "https://x/y", askedUser: "yes"), id: "design:d"),
            head + tail("design:d") + ["approve", "--link", "https://x/y", "--asked-user", "yes"])
        XCTAssertEqual(
            try args(.designChanges(askedUser: "no", link: nil), id: "design:d"),
            head + tail("design:d") + ["changes", "--asked-user", "no"])
        XCTAssertEqual(
            try args(.designChanges(askedUser: "no", link: "/a/b"), id: "design:d"),
            head + tail("design:d") + ["changes", "--asked-user", "no", "--link", "/a/b"])
        XCTAssertEqual(try args(.messageReply("ok"), id: "message:m"), head + tail("message:m") + ["reply", "ok"])
        XCTAssertEqual(try args(.agentPause(reason: nil), id: "agent:a"), head + tail("agent:a") + ["pause"])
        XCTAssertEqual(
            try args(.agentResume(reason: "go"), id: "agent:a"),
            head + tail("agent:a") + ["resume", "--reason", "go"])
        XCTAssertEqual(try args(.runPause(reason: nil), id: "run"), head + tail("run") + ["pause"])
        XCTAssertEqual(
            try args(.runResume(reason: "go"), id: "run"), head + tail("run") + ["resume", "--reason", "go"])
    }

    func testProjectPauseAndResumeSendTheRunRequestOverSsh() throws {
        let run = makeRequest(verb: .runPause(reason: nil), requestId: "run")
        XCTAssertEqual(
            try run.remoteArguments(),
            ["decide", "--project", "proj-1", "--json", "run", "--expect", testHash, "--nonce", testNonce,
             "--client", "Mac", "pause"])
        let ssh = try run.sshArguments(makeConfig())
        XCTAssertEqual(
            try ShParser.split(try XCTUnwrap(ssh.last)),
            ["cstan", "decide", "--project", "proj-1", "--json", "run", "--expect", testHash, "--nonce", testNonce,
             "--client", "Mac", "pause"])
        XCTAssertEqual(
            try makeRequest(verb: .runResume(reason: nil), requestId: "run").remoteArguments().suffix(2),
            ["resume"])
    }

    func testEveryWordRoundTripsTheShellParser() throws {
        let hostile = "it's a \"quote\" $(touch x) `id` ; & | > <"
        let verbs: [DecideVerb] = [
            .opDeny(note: hostile), .opApprove, .promptOption(3), .promptEsc,
            .designApprove(link: hostile, askedUser: hostile), .designChanges(askedUser: hostile, link: hostile),
            .messageReply(hostile), .agentPause(reason: hostile), .agentResume(reason: nil),
            .runPause(reason: hostile), .runResume(reason: hostile),
        ]
        let config = makeConfig()
        for verb in verbs {
            let request = makeRequest(verb: verb)
            let remoteArgs = try request.remoteArguments()
            let ssh = try request.sshArguments(config)
            let remote = try XCTUnwrap(ssh.last)
            XCTAssertEqual(try ShParser.split(remote), ["cstan"] + remoteArgs, "\(verb)")
        }
    }

    func testHostileNoteIsOneArgumentWithNewlineAsSpace() throws {
        let note = "it's $(touch x) and `id`\nsecond \"line\""
        let request = makeRequest(verb: .opDeny(note: note))
        let words = try ShParser.split(try XCTUnwrap(try request.sshArguments(makeConfig()).last))
        XCTAssertEqual(words.last, "it's $(touch x) and `id` second \"line\"")
        XCTAssertEqual(words.count, 1 + (try request.remoteArguments()).count)
    }

    func testIsDestructiveIsExactlyOpDenyAndRunPause() {
        XCTAssertTrue(DecideVerb.opDeny(note: nil).isDestructive)
        XCTAssertTrue(DecideVerb.runPause(reason: nil).isDestructive)
        let others: [DecideVerb] = [
            .opApprove, .promptOption(1), .promptEsc, .designApprove(link: "l", askedUser: "a"),
            .designChanges(askedUser: "a", link: nil), .messageReply("r"), .agentPause(reason: nil),
            .agentResume(reason: nil), .runResume(reason: nil),
        ]
        for verb in others { XCTAssertFalse(verb.isDestructive, "\(verb)") }
    }

    func testEveryVerbHasADocumentedRow() {
        let verbs: [DecideVerb] = [
            .opDeny(note: nil), .opApprove, .promptOption(1), .promptEsc, .designApprove(link: "l", askedUser: "a"),
            .designChanges(askedUser: "a", link: nil), .messageReply("r"), .agentPause(reason: nil),
            .agentResume(reason: nil), .runPause(reason: nil), .runResume(reason: nil),
        ]
        XCTAssertEqual(verbs.map(\.row), DecideVerbRow.documented)
    }

    func testTextStartingWithDashIsRefused() throws {
        for (verb, name) in [
            (DecideVerb.opDeny(note: "-x"), "note"), (.messageReply("--expect"), "reply"),
            (.agentPause(reason: "-r"), "reason"), (.designApprove(link: "-l", askedUser: "a"), "link"),
            (.designChanges(askedUser: "-a", link: nil), "answer"),
        ] {
            XCTAssertThrowsError(try makeRequest(verb: verb).remoteArguments()) {
                XCTAssertEqual($0 as? DecideRequestError, .textDash(name))
            }
        }
        // A dash inside the text, or after a leading space, is fine.
        XCTAssertNoThrow(try makeRequest(verb: .messageReply("a -b")).remoteArguments())
        XCTAssertNoThrow(try makeRequest(verb: .messageReply(" -b")).remoteArguments())
    }

    func testNonceNeverStartsWithDash() {
        for _ in 0..<2000 {
            let nonce = Nonce.make()
            XCTAssertEqual(nonce.utf8.count, Nonce.length)
            XCTAssertNotEqual(nonce.first, "-")
            XCTAssertTrue(Nonce.isValid(nonce))
        }
    }

    func testNonceStartingWithDashIsRefused() {
        XCTAssertFalse(Nonce.isValid("-abcdefghijklmnopqrstuv"))
        XCTAssertTrue(Nonce.isValid("abcdefghijklmnopqrstu-"))
        XCTAssertThrowsError(try makeRequest(verb: .opApprove, nonce: "-abcdefghijklmnopqrstuv").remoteArguments()) {
            XCTAssertEqual($0 as? DecideRequestError, .nonce)
        }
    }

    func testIdentifierStartingWithDashIsRefused() {
        XCTAssertThrowsError(try makeRequest(verb: .opApprove, requestId: "-op").remoteArguments()) {
            XCTAssertEqual($0 as? DecideRequestError, .idDash("request"))
        }
        XCTAssertThrowsError(try makeRequest(verb: .opApprove, projectId: "-p").remoteArguments()) {
            XCTAssertEqual($0 as? DecideRequestError, .idDash("project"))
        }
        XCTAssertNoThrow(try makeRequest(verb: .opApprove, requestId: "op:-a", projectId: "p-1").remoteArguments())
    }

    func testBlankOptionalTextMeansNone() throws {
        XCTAssertEqual(try args(.opDeny(note: "")), head + tail("op:p1") + ["deny"])
        XCTAssertEqual(try args(.opDeny(note: " \n\t ")), head + tail("op:p1") + ["deny"])
        XCTAssertEqual(try args(.runPause(reason: "  "), id: "run"), head + tail("run") + ["pause"])
        XCTAssertEqual(try args(.agentResume(reason: ""), id: "agent:a"), head + tail("agent:a") + ["resume"])
        XCTAssertThrowsError(try args(.messageReply(" "), id: "message:m"))
    }
}
