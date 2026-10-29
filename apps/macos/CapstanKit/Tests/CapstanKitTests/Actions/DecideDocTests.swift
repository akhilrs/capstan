import XCTest
@testable import CapstanKit

final class DecideDocTests: XCTestCase {
    /// The text of section 11, or a skip while the protocol document has none (plan-68 adds it).
    private func section11() throws -> String {
        let lines = try ProtocolDoc.load().components(separatedBy: "\n")
        guard let start = lines.firstIndex(where: { $0.hasPrefix("## 11.") }) else {
            throw XCTSkip("docs/remote-protocol.md has no section 11 yet; plan-68 decide-protocol adds it")
        }
        let rest = lines[(start + 1)...]
        let end = rest.firstIndex { $0.hasPrefix("## ") } ?? rest.endIndex
        return lines[start..<end].joined(separator: "\n")
    }

    func testVerbsTableEqualsDocumentedRows() throws {
        _ = try section11()
        let kinds = Set(DecideVerbRow.documented.map(\.kind))
        var rows: [DecideVerbRow] = []
        for table in try ProtocolDoc.tables(under: "## 11.") {
            for cells in table where cells.count == 3 && kinds.contains(cells[0].trimmingCharacters(in: .whitespaces)) {
                rows.append(DecideVerbRow(
                    kind: cells[0].trimmingCharacters(in: .whitespaces),
                    verb: cells[1].trimmingCharacters(in: .whitespaces),
                    outcome: cells[2].trimmingCharacters(in: .whitespaces)))
            }
        }
        XCTAssertEqual(rows.count, DecideVerbRow.documented.count)
        XCTAssertEqual(Set(rows.map { "\($0.kind) \($0.verb) \($0.outcome)" }),
                       Set(DecideVerbRow.documented.map { "\($0.kind) \($0.verb) \($0.outcome)" }))
    }

    func testEveryCodeIsInTheDocAndDecodes() throws {
        let text = try section11()
        for wire in DecideErrorCode.wireNames {
            XCTAssertTrue(text.contains(wire), "section 11 does not mention \(wire)")
            if case .other = DecideErrorCode(wire: wire) { XCTFail("\(wire) does not decode") }
        }
    }

    func testJSONExamplesDecode() throws {
        _ = try section11()
        let blocks = try ProtocolDoc.jsonBlocks().filter { $0.section == 11 }
        XCTAssertFalse(blocks.isEmpty)
        for block in blocks {
            XCTAssertNoThrow(try DecideAnswer(data: Data(block.text.utf8)), block.text)
        }
    }
}
