import XCTest
@testable import CapstanKit

final class ApproveAsTests: XCTestCase {
    private let yes = HostCapabilities(reachable: true, events: true, decide: true)

    private func item(_ choices: String?, kind: String = "op", needsTerminal: Bool = false) throws -> WaitingItem {
        var members = [
            "\"key\":\"op:1\"", "\"requestId\":\"op:p1\"", "\"kind\":\"\(kind)\"", "\"since\":\"t\"",
            "\"summary\":\"git log\"", "\"contentHash\":\"\(testHash)\"", "\"needsTerminal\":\(needsTerminal)",
        ]
        if let choices { members.append("\"approveChoices\":\(choices)") }
        return try WaitingItem(json: try decodeJSON(Data(("{" + members.joined(separator: ",") + "}").utf8)))
    }

    private let full = """
        {"once":true,"exact":true,"prefixes":["git log","git log --oneline"],"auto":["hour","session"],\
        "limit":null,"floorEntry":null}
        """

    private func limited(_ limit: String, once: Bool = true, entry: String? = nil) -> String {
        let floor = entry.map { "\"\($0)\"" } ?? "null"
        return "{\"once\":\(once),\"exact\":false,\"prefixes\":[],\"auto\":[],\"limit\":\"\(limit)\",\"floorEntry\":\(floor)}"
    }

    // MARK: Decode

    func testDecodesFullChoices() throws {
        let choices = try XCTUnwrap(try item(full).approveChoices)
        XCTAssertTrue(choices.once)
        XCTAssertTrue(choices.exact)
        XCTAssertEqual(choices.prefixes.map(\.raw), ["git log", "git log --oneline"])
        XCTAssertEqual(choices.auto, [.hour, .session])
        XCTAssertNil(choices.limit)
        XCTAssertNil(choices.floorEntry)
    }

    func testEachLimitDecodes() throws {
        let cases: [(String, ApproveChoices.Limit)] = [
            ("floor", .floor), ("restart", .restart), ("truncated", .truncated), ("not_checked", .notChecked),
            ("text_mismatch", .textMismatch), ("later_reason", .other("later_reason")),
        ]
        for (wire, expected) in cases {
            let choices = try XCTUnwrap(try item(limited(wire, once: wire != "text_mismatch")).approveChoices)
            XCTAssertEqual(choices.limit, expected, wire)
        }
        let floor = try XCTUnwrap(try item(limited("floor", entry: "rm -rf")).approveChoices)
        XCTAssertEqual(floor.floorEntry, "rm -rf")
    }

    func testUnknownAutoKindIsKept() throws {
        let json = "{\"once\":true,\"exact\":true,\"prefixes\":[],\"auto\":[\"day\"],\"limit\":null,\"floorEntry\":null}"
        XCTAssertEqual(try item(json).approveChoices?.auto, [.other("day")])
    }

    func testAbsentOrNullChoicesAreNil() throws {
        XCTAssertNil(try item(nil).approveChoices)
        XCTAssertNil(try item("null").approveChoices)
        XCTAssertNil(try item(nil, kind: "message").approveChoices)
    }

    func testMalformedChoicesAreRefused() throws {
        XCTAssertThrowsError(try item("{\"once\":true}"))
        XCTAssertThrowsError(try item("{\"once\":true,\"exact\":true,\"prefixes\":[1],\"auto\":[]}"))
    }

    func testStoredItemKeepsChoices() throws {
        let original = try item(limited("floor", entry: "rm"))
        let stored = StoredWaiting(original)
        XCTAssertEqual(try WaitingItem(json: stored.json).approveChoices, original.approveChoices)
        let data = try JSONEncoder().encode(stored)
        XCTAssertEqual(try JSONDecoder().decode(StoredWaiting.self, from: data).approveChoices, original.approveChoices)
    }

    // MARK: Argv

    private let head = ["decide", "--project", "proj-1", "--json", "op:p1", "--expect", testHash, "--nonce", testNonce,
                        "--client", "Mac"]

    private func args(_ choice: ApproveChoice) throws -> [String] {
        try makeRequest(verb: .opApproveAs(choice)).remoteArguments()
    }

    func testArgvForEveryChoice() throws {
        XCTAssertEqual(try args(.once), head + ["approve-as", "once"])
        XCTAssertEqual(try args(.exact), head + ["approve-as", "exact"])
        XCTAssertEqual(try args(.autoHour), head + ["approve-as", "auto-hour"])
        XCTAssertEqual(try args(.autoSession), head + ["approve-as", "auto-session"])
        XCTAssertEqual(try args(.prefix(raw: "git log")), head + ["approve-as", "prefix", "--prefix", "git log"])
    }

    func testPrefixPassesByteEqual() throws {
        for raw in ["git  log\u{00A0}x", "echo héllo ✓ 日本", "a\tb", " lead", "e\u{0301}"] {
            let words = try args(.prefix(raw: raw))
            XCTAssertEqual(Array(words.last!.utf8), Array(raw.utf8), raw)
        }
    }

    func testBadPrefixIsRefusedBeforeSending() throws {
        XCTAssertThrowsError(try args(.prefix(raw: ""))) { XCTAssertEqual($0 as? DecideRequestError, .text("prefix")) }
        XCTAssertThrowsError(try args(.prefix(raw: "-rf"))) {
            XCTAssertEqual($0 as? DecideRequestError, .textDash("prefix"))
        }
    }

    func testVerbRowAndDestructiveness() {
        XCTAssertEqual(DecideVerb.opApproveAs(.once).row, DecideVerbRow(kind: "op", verb: "approve-as", outcome: "applied"))
        XCTAssertTrue(DecideVerbRow.documented.contains(DecideVerb.opApproveAs(.exact).row))
    }

    // MARK: Availability

    private func avail(_ verb: DecideVerb, _ item: WaitingItem?) -> DecideAvailability {
        availability(verb, capabilities: yes, item: item, option: nil)
    }

    func testApproveAsOnlyWhenListed() throws {
        let op = try item(full)
        for choice in [ApproveChoice.once, .exact, .prefix(raw: "git log"), .prefix(raw: "git log --oneline"), .autoHour,
                       .autoSession] {
            XCTAssertEqual(avail(.opApproveAs(choice), op), .available, "\(choice)")
        }
        guard case .hidden = avail(.opApproveAs(.prefix(raw: "git")), op) else { return XCTFail("unlisted prefix") }
        // Same scalars, different bytes: not the offered prefix.
        guard case .hidden = avail(.opApproveAs(.prefix(raw: "git log ")), op) else { return XCTFail("near prefix") }
        let onlyOnce = try item(limited("restart"))
        XCTAssertEqual(avail(.opApproveAs(.once), onlyOnce), .available)
        for choice in [ApproveChoice.exact, .autoHour, .autoSession, .prefix(raw: "git log")] {
            guard case .hidden = avail(.opApproveAs(choice), onlyOnce) else { return XCTFail("\(choice)") }
        }
    }

    func testByteEqualityIgnoresCanonicalEquivalence() throws {
        let composed = "caf\u{00E9}"
        let decomposed = "cafe\u{0301}"
        let json = "{\"once\":true,\"exact\":true,\"prefixes\":[\"\(composed)\"],\"auto\":[],\"limit\":null,\"floorEntry\":null}"
        let op = try item(json)
        XCTAssertEqual(avail(.opApproveAs(.prefix(raw: composed)), op), .available)
        guard case .hidden = avail(.opApproveAs(.prefix(raw: decomposed)), op) else { return XCTFail() }
    }

    func testOldHostFallsBackToForward() throws {
        let old = try item(nil)
        XCTAssertEqual(avail(.opApprove, old), .available)
        guard case .hidden = avail(.opApproveAs(.once), old) else { return XCTFail() }
        // A host that sends choices does not also get the forward.
        guard case .hidden = avail(.opApprove, try item(full)) else { return XCTFail() }
    }

    func testTextMismatchOffersDenyOnly() throws {
        let mismatch = try item(limited("text_mismatch", once: true))
        guard case .hidden = avail(.opApprove, mismatch) else { return XCTFail() }
        guard case .hidden = avail(.opApproveAs(.once), mismatch) else { return XCTFail() }
        XCTAssertEqual(avail(.opDeny(note: nil), mismatch), .available)
    }

    func testApproveAsNeedsOpItemAndDecideCapability() throws {
        guard case .hidden = avail(.opApproveAs(.once), nil) else { return XCTFail() }
        guard case .hidden = avail(.opApproveAs(.once), try item(full, kind: "message")) else { return XCTFail() }
        let off = HostCapabilities(reachable: true, events: true, decide: false)
        guard case .hidden = availability(.opApproveAs(.once), capabilities: off, item: try item(full), option: nil)
        else { return XCTFail() }
        guard case .terminalOnly = avail(.opApproveAs(.once), try item(full, needsTerminal: true)) else {
            return XCTFail()
        }
    }

    // MARK: Rows

    func testRowsAreOrderedWithReasons() throws {
        let rows = ApproveOptions.rows(try item(full))
        XCTAssertEqual(
            rows.map(\.choice),
            [.once, .exact, .prefix(raw: "git log"), .prefix(raw: "git log --oneline"), .autoHour, .autoSession])
        XCTAssertTrue(rows.allSatisfy { $0.enabled && $0.limit == nil })

        let held = ApproveOptions.rows(try item(limited("floor", entry: "sudo")))
        XCTAssertEqual(held.map(\.choice), [.once, .exact, .autoHour, .autoSession])
        XCTAssertEqual(held.map(\.enabled), [true, false, false, false])
        XCTAssertTrue(held.allSatisfy { $0.limit == .floor && $0.floorEntry == "sudo" })

        let mismatch = ApproveOptions.rows(try item(limited("text_mismatch", once: true)))
        XCTAssertTrue(mismatch.allSatisfy { !$0.enabled && $0.limit == .textMismatch })

        XCTAssertTrue(ApproveOptions.rows(try item(nil)).isEmpty)
    }

    // MARK: Result

    func testResultDecodesApproval() throws {
        let line = """
            {"v":1,"decisionId":"remote-0000000005","requestId":"op:op-4","nonce":"\(testNonce)","outcome":"applied",\
            "repeat":false,"kind":"op","verb":"approve-as","contentHash":"\(testHash)","eventSeq":61,\
            "approval":{"choice":"exact","grantId":"grant-3","windowId":null,"expiresAt":"2026-10-01T10:01:00.000Z"}}
            """
        guard case .result(let result) = try DecideAnswer(data: Data(line.utf8)) else { return XCTFail() }
        XCTAssertEqual(
            result.approval,
            ApprovalResult(choice: "exact", grantId: "grant-3", windowId: nil, expiresAt: "2026-10-01T10:01:00.000Z"))
        let once = line.replacingOccurrences(
            of: "{\"choice\":\"exact\",\"grantId\":\"grant-3\",\"windowId\":null,\"expiresAt\":\"2026-10-01T10:01:00.000Z\"}",
            with: "{\"choice\":\"once\",\"grantId\":null,\"windowId\":null,\"expiresAt\":null}")
        guard case .result(let plain) = try DecideAnswer(data: Data(once.utf8)) else { return XCTFail() }
        XCTAssertEqual(plain.approval, ApprovalResult(choice: "once"))
    }

    func testResultWithoutApprovalIsNil() throws {
        guard case .result(let result) = try DecideAnswer(data: Data(resultLine().utf8)) else { return XCTFail() }
        XCTAssertNil(result.approval)
    }
}
