import CapstanKit
import XCTest

@testable import CapstanAppCore

final class MiscTests: XCTestCase {
    private let hostA = ShellFixtures.hostA
    private let hostB = ShellFixtures.hostB

    private func groups() -> [InboxGroup] {
        let ops = [
            ShellFixtures.row(host: hostA, project: "p1", key: "a", kind: .op),
            ShellFixtures.row(host: hostA, project: "p2", key: "b", kind: .op),
        ]
        let prompts = [ShellFixtures.row(host: hostB, project: "p1", key: "c", kind: .prompt)]
        return [
            InboxGroup(kind: .op, title: "Command proposals", rows: ops),
            InboxGroup(kind: .prompt, title: "Prompts", rows: prompts),
        ]
    }

    func test_InboxScope_byKind() {
        let result = InboxScope(kind: .prompt).apply(groups())
        XCTAssertEqual(result.map(\.kind), [.prompt])
        XCTAssertEqual(result.first?.count, 1)
    }

    func test_InboxScope_byProject() {
        let result = InboxScope(hostId: hostA, projectId: "p2").apply(groups())
        XCTAssertEqual(result.count, 1)
        XCTAssertEqual(result.first?.rows.map(\.id.key), ["b"])
        XCTAssertEqual(result.first?.count, 1)
    }

    func test_InboxScope_byKindAndProject() {
        XCTAssertEqual(InboxScope(kind: .op, hostId: hostA, projectId: "p1").apply(groups()).first?.rows.map(\.id.key), ["a"])
        XCTAssertTrue(InboxScope(kind: .prompt, hostId: hostA, projectId: "p1").apply(groups()).isEmpty)
    }

    func test_TextBudget_usedMultibyte() {
        let budget = TextBudget(limit: 10)
        XCTAssertEqual(budget.used("a"), 1)
        XCTAssertEqual(budget.used("é"), 2)
        XCTAssertEqual(budget.used("€"), 3)
        XCTAssertEqual(budget.used("😀"), 4)
    }

    func test_TextBudget_allowsMultibyte() {
        let budget = TextBudget(limit: 6)
        XCTAssertTrue(budget.allows("€€"))
        XCTAssertFalse(budget.allows("€€a"))
        XCTAssertTrue(TextBudget.reason.allows(String(repeating: "a", count: ProtocolLimits.reasonCap)))
        XCTAssertFalse(TextBudget.reason.allows(String(repeating: "a", count: ProtocolLimits.reasonCap + 1)))
    }

    func test_TextBudget_displayPrefixAt4096() {
        let text = String(repeating: "a", count: 4096)
        let result = TextBudget.displayPrefix(text, maxBytes: 4096)
        XCTAssertEqual(result.text, text)
        XCTAssertFalse(result.cut)
    }

    func test_TextBudget_displayPrefixAt4097() {
        let text = String(repeating: "a", count: 4097)
        let result = TextBudget.displayPrefix(text, maxBytes: 4096)
        XCTAssertEqual(result.text.utf8.count, 4096)
        XCTAssertTrue(result.cut)
    }

    func test_TextBudget_displayPrefixFourByteCharacterAtLimit() {
        let text = String(repeating: "a", count: 4094) + "😀"
        let result = TextBudget.displayPrefix(text, maxBytes: 4096)
        XCTAssertEqual(result.text, String(repeating: "a", count: 4094))
        XCTAssertTrue(result.cut)
        let fits = String(repeating: "a", count: 4092) + "😀"
        XCTAssertEqual(TextBudget.displayPrefix(fits, maxBytes: 4096).text, fits)
    }

    func test_PendingRow_select() {
        let id = InboxRowID(hostId: hostA, projectId: "p1", key: "a")
        let pending = PendingRow(row: id, since: Date(timeIntervalSince1970: 0))
        XCTAssertEqual(pending.resolve(rows: [id], now: Date(timeIntervalSince1970: 100)), .select)
    }

    func test_PendingRow_waitAt29point9() {
        let id = InboxRowID(hostId: hostA, projectId: "p1", key: "a")
        let pending = PendingRow(row: id, since: Date(timeIntervalSince1970: 0))
        XCTAssertEqual(pending.resolve(rows: [], now: Date(timeIntervalSince1970: 29.9)), .wait)
    }

    func test_PendingRow_expireAt30() {
        let id = InboxRowID(hostId: hostA, projectId: "p1", key: "a")
        let pending = PendingRow(row: id, since: Date(timeIntervalSince1970: 0))
        XCTAssertEqual(pending.resolve(rows: [], now: Date(timeIntervalSince1970: 30)), .expire)
    }

    private let ink = (r: 0.1, g: 0.2, b: 0.3)

    func test_TokenValue_hex() throws {
        let value = try XCTUnwrap(TokenValue.parse("#ff8000", ink: ink))
        XCTAssertEqual(value.r, 1, accuracy: 0.0001)
        XCTAssertEqual(value.g, 128.0 / 255, accuracy: 0.0001)
        XCTAssertEqual(value.b, 0, accuracy: 0.0001)
        XCTAssertEqual(value.a, 1)
        XCTAssertNil(TokenValue.parse("#ff80", ink: ink))
        XCTAssertNil(TokenValue.parse("#gg0000", ink: ink))
    }

    func test_TokenValue_rgba() throws {
        let value = try XCTUnwrap(TokenValue.parse("rgba(252,252,250,.66)", ink: ink))
        XCTAssertEqual(value.r, 252.0 / 255, accuracy: 0.0001)
        XCTAssertEqual(value.b, 250.0 / 255, accuracy: 0.0001)
        XCTAssertEqual(value.a, 0.66, accuracy: 0.0001)
    }

    func test_TokenValue_percentInk() throws {
        let value = try XCTUnwrap(TokenValue.parse("4% ink", ink: ink))
        XCTAssertEqual(value.r, 0.1, accuracy: 0.0001)
        XCTAssertEqual(value.g, 0.2, accuracy: 0.0001)
        XCTAssertEqual(value.b, 0.3, accuracy: 0.0001)
        XCTAssertEqual(value.a, 0.04, accuracy: 0.0001)
    }

    func test_TokenValue_percentWhite() throws {
        let value = try XCTUnwrap(TokenValue.parse("9% white", ink: ink))
        XCTAssertEqual(value.r, 1)
        XCTAssertEqual(value.g, 1)
        XCTAssertEqual(value.b, 1)
        XCTAssertEqual(value.a, 0.09, accuracy: 0.0001)
        XCTAssertNil(TokenValue.parse("4% / 8% ink", ink: ink))
    }
}
