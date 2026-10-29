import XCTest
@testable import CapstanKit

final class DocConformanceTests: XCTestCase {
    private func blocks(_ section: Int) throws -> [String] {
        try ProtocolDoc.jsonBlocks().filter { $0.section == section }.map(\.text)
    }

    private func lines(_ block: String) -> [String] {
        block.components(separatedBy: "\n").filter { !$0.isEmpty }
    }

    func testSectionTwoLines() throws {
        let all = try blocks(2).flatMap(lines)
        XCTAssertEqual(all.count, 3)
        let decoder = LineDecoder()
        let decoded = try all.map { try decoder.decode(Data($0.utf8)) }
        guard case .hello(let hello) = decoded[0] else { return XCTFail("hello") }
        XCTAssertEqual(hello.version, "0.1.0")
        XCTAssertEqual(hello.host, "build-host")
        XCTAssertEqual(hello.projects, 2)
        guard case .project(let ready) = decoded[1] else { return XCTFail("project") }
        XCTAssertEqual(ready.projectId, "proj-a")
        XCTAssertEqual(ready.root, "/srv/work/alpha")
        XCTAssertEqual(ready.state, .ready)
        XCTAssertEqual(ready.epoch, "daeca3b2245f3519")
        XCTAssertEqual(ready.head, 26)
        guard case .project(let silent) = decoded[2] else { return XCTFail("project") }
        XCTAssertEqual(silent.projectId, "proj-b")
        XCTAssertEqual(silent.state, .silent)
        XCTAssertNil(silent.epoch)
        XCTAssertNil(silent.head)
    }

    func testSectionThreeCursor() throws {
        let found = try blocks(3)
        XCTAssertEqual(found.count, 2)
        guard case .cursor(let cursor) = try LineDecoder().decode(Data(found[0].utf8)) else {
            return XCTFail("cursor")
        }
        let payload = try cursor.decoded()
        let expected = try CursorPayload(json: try decodeJSON(Data(found[1].utf8)))
        XCTAssertEqual(payload, expected)
        XCTAssertEqual(payload.p["proj-a"]?.k, 0)
        XCTAssertEqual(payload.p["proj-b"]?.k, 1)
        XCTAssertEqual(payload.p["proj-b"]?.d, 1_790_000_000)
        XCTAssertEqual(Cursor(payload: payload), cursor)
        XCTAssertEqual(Cursor(payload: payload).text, cursor.text)
        let json = try decodeJSON(Data(found[0].utf8))
        XCTAssertEqual(try json.string("cursor"), cursor.text)
    }

    func testSectionFiveEventAndPages() throws {
        let found = try blocks(5)
        XCTAssertEqual(found.count, 5)
        guard case .event(let projectId, let event) = try LineDecoder().decode(Data(found[0].utf8)) else {
            return XCTFail("event")
        }
        XCTAssertEqual(projectId, "prustonly")
        XCTAssertEqual(event.type, "agent.registered")
        XCTAssertEqual(event.body.effects.count, 1)
        XCTAssertEqual(event.body.effects[0].op, .upsert)
        XCTAssertEqual(event.body.effects[0].section, "agents")
        XCTAssertEqual(event.body.effects[0].key, "dev-3")
        XCTAssertEqual(event.body.agentBody?.agentId, "dev-3")
        XCTAssertNil(event.body.promptBody)

        let waiting = try Page<WaitingItem>(data: Data(found[1].utf8))
        XCTAssertEqual(waiting.head, 26)
        XCTAssertEqual(waiting.items.first?.key, "message:3")
        XCTAssertEqual(waiting.items.first?.kind, .message)
        XCTAssertEqual(waiting.items.first?.summary.raw, "Please look at plan 1")
        XCTAssertNil(waiting.next)

        let agents = try Page<AgentItem>(data: Data(found[2].utf8))
        XCTAssertEqual(agents.items.first?.agentId, "dev-1")
        XCTAssertNil(agents.items.first?.task)

        let plans = try Page<PlanItem>(data: Data(found[3].utf8))
        XCTAssertEqual(plans.items.first?.packages, PackageCounts(total: 3, assigned: 2, reported: 1, integrated: 0))
        XCTAssertEqual(plans.items.first?.title.raw, "Remote protocol")

        let run = try RunItem(json: try decodeJSON(Data(found[4].utf8)))
        XCTAssertEqual(run.requestId, "run")
        XCTAssertEqual(run.state, "active")
        XCTAssertFalse(run.paused)
    }

    func testSectionTenAnswers() throws {
        let found = try blocks(10)
        XCTAssertEqual(found.count, 4)
        let hash = try HashAnswer(json: try decodeJSON(Data(found[0].utf8)))
        XCTAssertEqual(hash.requestId, "message:msg-1")
        XCTAssertNotNil(hash.contentHash)
        let gone = try HashAnswer(json: try decodeJSON(Data(found[1].utf8)))
        XCTAssertNil(gone.contentHash)
        let item = try ItemAnswer(json: try decodeJSON(Data(found[2].utf8)))
        guard case .agent(let agent)? = item.item else { return XCTFail("agent item") }
        XCTAssertEqual(agent.agentId, "dev-1")
        let removed = try ItemAnswer(json: try decodeJSON(Data(found[3].utf8)))
        XCTAssertNil(removed.item)
        XCTAssertEqual(removed.key, "message:9")
    }

    func testEveryBlockDecodes() throws {
        let all = try ProtocolDoc.jsonBlocks()
        XCTAssertEqual(all.count, 12)
        for block in all {
            for line in lines(block.text) {
                let value = try decodeJSON(Data(line.utf8))
                if value["kind"] != nil {
                    let decoded = try LineDecoder().decode(Data(line.utf8))
                    if case .unknownKind = decoded { XCTFail("unknown kind in section \(block.section)") }
                }
            }
        }
    }

    // MARK: drift

    func testLineKindsAndProjectStates() throws {
        let tables = try ProtocolDoc.tables(under: "## 2.")
        XCTAssertEqual(tables.count, 2)
        XCTAssertEqual(tables[0].map { $0[0] }, StreamLine.wireKinds)
        XCTAssertEqual(tables[1].map { $0[0] }, ProjectAvailability.wireNames)
    }

    func testResyncReasons() throws {
        let doc = try ProtocolDoc.load()
        for reason in ResyncReason.wireNames {
            XCTAssertTrue(doc.contains("`\(reason)`"), reason)
        }
    }

    func testEventTypes() throws {
        let rows = try ProtocolDoc.tableRows(under: "## 5.")
        XCTAssertEqual(Set(rows.map { $0[0] }), Set(FeedEventType.known))
        XCTAssertEqual(rows.count, FeedEventType.known.count)
    }

    func testCaps() throws {
        let caps = try ProtocolDoc.tableRows(under: "## 8.")
        func cap(_ name: String) -> Int? {
            caps.first { $0[0] == name }.flatMap { ProtocolDoc.number($0[1]) }
        }
        XCTAssertEqual(cap("event_feed::TEXT_CAP"), ProtocolLimits.textCap)
        XCTAssertEqual(cap("event_feed::REASON_CAP"), ProtocolLimits.reasonCap)
        XCTAssertEqual(cap("event_feed::OPTIONS_CAP"), ProtocolLimits.optionsCap)
        XCTAssertEqual(cap("event_feed::OPTION_TEXT_CAP"), ProtocolLimits.optionTextCap)
        XCTAssertEqual(cap("snapshot_feed::TEXT_CAP"), ProtocolLimits.snapshotTextCap)
    }

    func testLimits() throws {
        let rows = try ProtocolDoc.tableRows(under: "## 10.")
        func limit(_ name: String) -> Int? {
            rows.first { $0[0].hasPrefix(name) }.flatMap { ProtocolDoc.number($0[1]) }
        }
        XCTAssertEqual(limit("bytes of one event"), ProtocolLimits.eventBytes)
        XCTAssertEqual(limit("bytes of one stream line"), ProtocolLimits.lineBytes)
        XCTAssertEqual(limit("bytes of one stream line"), LineDecoder.maxLineBytes)
        XCTAssertEqual(limit("minimum poll interval"), ProtocolLimits.minPollMs)
        XCTAssertEqual(limit("serialized bytes per page"), ProtocolLimits.pageBytes)
        XCTAssertEqual(limit("event_feed::MAX_LIMIT"), ProtocolLimits.eventsPerPage)
        XCTAssertEqual(limit("snapshot_feed::MAX_LIMIT"), ProtocolLimits.itemsPerPage)
        XCTAssertEqual(limit("event_feed::MAX_SCAN_ROWS"), ProtocolLimits.eventRowReads)
        XCTAssertEqual(limit("snapshot_feed::MAX_READS"), ProtocolLimits.itemRowReads)
    }
}
