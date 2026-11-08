import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class AgentRowModelTests: XCTestCase {
    private func row(model: UntrustedText?, source: String?) -> AgentRow {
        let agent = StoredAgent(
            key: "dev-1", agentId: "dev-1", kind: "Developer", role: "developer", generation: 1, state: "active",
            paused: false, task: nil, requestId: "agent:dev-1", contentHash: "h", model: model, modelSource: source)
        return AgentRow(agent: agent, health: .working)
    }

    private func text(_ raw: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: false)
    }

    func testAModelWithASourceShowsBoth() {
        let r = row(model: text("claude-opus-5-5"), source: "plan")
        XCTAssertEqual(r.model, "claude-opus-5-5")
        XCTAssertEqual(r.modelLabel?.short, "plan")
        XCTAssertEqual(r.modelSpoken, ", model claude-opus-5-5, plan override")
    }

    func testNoModelShowsNothingEvenWithASource() {
        let r = row(model: nil, source: "plan")
        XCTAssertNil(r.model)
        XCTAssertNil(r.modelLabel)
        XCTAssertEqual(r.modelSpoken, "")
    }

    func testAModelWithoutASourceReadsOther() {
        XCTAssertEqual(row(model: text("gpt-5.1-codex"), source: nil).modelLabel?.short, "other")
    }

    func testControlCharactersAreCleanedAndAnEmptyIdIsNoModel() {
        XCTAssertEqual(row(model: text("a\nb"), source: "host").model, "a b")
        XCTAssertNil(row(model: text("\u{1}\u{2}"), source: "host").model)
        XCTAssertNil(row(model: text("x", verified: false), source: "host").model)
    }

    func testALongIdIsCappedAt100Bytes() {
        let r = row(model: text(String(repeating: "m", count: 300)), source: "spawn")
        XCTAssertEqual(r.model?.utf8.count, 100)
    }
}
