import CapstanKit
import XCTest

@testable import CapstanAppCore

final class InboxTests: XCTestCase {
    private func options(_ specs: [(Int, Bool, Bool)]) throws -> [PromptOption] {
        let list = specs.map { number, accepts, widens in
            "{\"number\":\(number),\"text\":\"Option \(number)\",\"acceptsText\":\(accepts),\"widensPermissions\":\(widens)}"
        }.joined(separator: ",")
        let body = try ShellFixtures.decode(
            JSONValue.self,
            "{\"relayId\":\"r1\",\"agentId\":\"a1\",\"state\":\"waiting\",\"text\":\"Q\",\"options\":[\(list)],\"effects\":[]}")
        let detail = ItemDetail(key: "k", eventType: "prompt.waiting", seq: 1, body: body)
        return try XCTUnwrap(detail.promptBody).options
    }

    func test_OptionRows_plainWideningAndText() throws {
        let rows = OptionRows.make(try options([(1, false, false), (2, false, true), (3, true, false), (4, true, true)]), needsTerminal: false)
        XCTAssertEqual(rows.map(\.state), [.selectable, .widening, .text, .widening])
        XCTAssertNil(rows[0].note)
        XCTAssertNotNil(rows[1].note)
        XCTAssertNotNil(rows[2].note)
    }

    func test_OptionRows_needsTerminalHasNoSelectableRow() throws {
        let rows = OptionRows.make(try options([(1, false, false), (2, false, true)]), needsTerminal: true)
        XCTAssertEqual(rows.map(\.state), [.text, .widening])
        XCTAssertFalse(rows.contains { $0.state == .selectable })
    }

    func test_RowPills_eachFlag() {
        XCTAssertEqual(RowPills.make(needsTerminal: true, forwarded: false, stale: false), [.terminalOnly])
        XCTAssertEqual(RowPills.make(needsTerminal: false, forwarded: true, stale: false), [.forwarded])
        XCTAssertEqual(RowPills.make(needsTerminal: false, forwarded: false, stale: true), [.stale])
        XCTAssertEqual(RowPills.make(needsTerminal: false, forwarded: false, stale: false), [])
        XCTAssertEqual(RowPills.make(needsTerminal: true, forwarded: true, stale: true), [.terminalOnly, .forwarded, .stale])
    }

    func test_RowPills_forwardedKeptForSameHashAndDroppedForNew() throws {
        let item = try ShellFixtures.waiting(key: "k", hash: ShellFixtures.hashA)
        let record = ForwardRecord(requestId: "k", contentHash: ShellFixtures.hashA, messageId: nil, at: Date())
        let same = ForwardPill.shown(item: item, record: record)
        XCTAssertEqual(RowPills.make(needsTerminal: false, forwarded: same, stale: false), [.forwarded])
        let changed = try ShellFixtures.waiting(key: "k", hash: ShellFixtures.hashB)
        let other = ForwardPill.shown(item: changed, record: record)
        XCTAssertEqual(RowPills.make(needsTerminal: false, forwarded: other, stale: false), [])
    }

    func test_InspectorMode() {
        XCTAssertEqual(InspectorMode.make(decide: true, kind: .op), .interactive)
        XCTAssertEqual(InspectorMode.make(decide: false, kind: .prompt), .readOnlyNoDecide)
        XCTAssertEqual(InspectorMode.make(decide: true, kind: .integration), .readOnlyIntegration)
        XCTAssertEqual(InspectorMode.make(decide: false, kind: .integration), .readOnlyIntegration)
        XCTAssertEqual(InspectorMode.readOnlyNoDecide.terminalHint, "Answer in the terminal")
        XCTAssertEqual(InspectorMode.readOnlyIntegration.terminalHint, "Merge from the terminal")
        XCTAssertNil(InspectorMode.interactive.terminalHint)
    }

    func test_LinkCheck() {
        XCTAssertNotNil(LinkCheck.openable("https://example.org/a?b=1"))
        XCTAssertNotNil(LinkCheck.openable("http://example.org"))
        XCTAssertNil(LinkCheck.openable("javascript:alert(1)"))
        XCTAssertNil(LinkCheck.openable("file:///etc/passwd"))
        XCTAssertNil(LinkCheck.openable("data:text/html,hi"))
        XCTAssertNil(LinkCheck.openable("https://"))
        XCTAssertNil(LinkCheck.openable("https://exa mple.org"))
        XCTAssertNil(LinkCheck.openable(""))
    }

    func test_ComposerState() {
        let budget = TextBudget(limit: ProtocolLimits.reasonCap)
        XCTAssertEqual(ComposerState.check("  \n", budget: budget), .empty)
        XCTAssertEqual(ComposerState.check("ok", budget: budget), .ready)
        XCTAssertEqual(ComposerState.check(String(repeating: "a", count: 1024), budget: budget), .ready)
        XCTAssertEqual(ComposerState.check(String(repeating: "a", count: 1025), budget: budget), .overLimit)
        XCTAssertEqual(ComposerState.check(String(repeating: "\u{00E9}", count: 513), budget: budget), .overLimit)
    }

    func test_AgeText() {
        let now = ISO8601DateFormatter().date(from: "2026-10-29T12:00:00Z")!
        XCTAssertEqual(AgeText.short(since: "2026-10-29T11:55:00Z", now: now), "5m")
        XCTAssertEqual(AgeText.short(since: "2026-10-29T09:00:00Z", now: now), "3h")
        XCTAssertEqual(AgeText.short(since: "2026-10-27T12:00:00Z", now: now), "2d")
        XCTAssertEqual(AgeText.short(since: "2026-10-29T11:59:50Z", now: now), "now")
        XCTAssertEqual(AgeText.short(since: "bad", now: now), "")
    }
}
