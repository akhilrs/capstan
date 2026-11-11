import Foundation
import XCTest
@testable import CapstanKit

final class SetInboxTests: XCTestCase {
    private let sha = "3f2a9c1d5b7e4a60918c2d4e6f8091a2b3c4d5e6"

    private func memberJSON(_ repo: String, _ position: Int, _ state: String) -> String {
        "{\"repo\":\"\(repo)\",\"position\":\(position),\"state\":\"\(state)\",\"dropped\":false}"
    }

    /// An integration item of the set `set-4` for `repo`; the set has api (in main), web and worker (built).
    private func item(_ repo: String, position: Int, since: String) -> String {
        let members = [memberJSON("api", 0, "confirmed"), memberJSON("web", 1, "merged"), memberJSON("worker", 2, "merged")]
        let set = "{\"setId\":\"set-4\",\"state\":\"merged\",\"repo\":\"\(repo)\",\"position\":\(position),\"attempt\":1,"
            + "\"reviewSubject\":false,\"dropped\":false,\"members\":[\(members.joined(separator: ","))]}"
        return "{\"key\":\"integration:\(repo)\",\"requestId\":\"integration:\(repo)\",\"kind\":\"integration\","
            + "\"since\":\"\(since)\",\"summary\":\"merged b (repo \(repo) of set set-4)\",\"contentHash\":\"h\","
            + "\"needsTerminal\":false,\"integrationState\":\"merged\",\"branch\":\"b\",\"headSha\":\"\(sha)\",\"set\":\(set)}"
    }

    private func harness(_ repos: [(String, Int, String)], extra: Bool = true) throws -> ReducerHarness {
        var harness = ReducerHarness()
        harness.apply(try StoreWire.hello(), try StoreWire.project("proj-a"))
        for (repo, position, since) in repos {
            harness.apply(try StoreWire.snapshot(
                "proj-a", "waiting", key: "integration:\(repo)", item: item(repo, position: position, since: since)))
        }
        if extra {
            harness.apply(try StoreWire.snapshot(
                "proj-a", "waiting", key: "integration:solo", item: StoreWire.waitingItem("integration:solo")))
        }
        harness.apply(try StoreWire.end("proj-a"))
        return harness
    }

    func testTheMembersOfASetAreOneRowAndTheNextMemberStands() throws {
        let h = try harness([
            ("api", 0, "2026-10-01T09:00:00.000Z"), ("web", 1, "2026-10-01T09:10:00.000Z"),
            ("worker", 2, "2026-10-01T09:20:00.000Z"),
        ])
        let rows = inbox([h.host]).flatMap(\.rows)
        let setRows = rows.filter { $0.set != nil }
        XCTAssertEqual(setRows.count, 1)
        XCTAssertEqual(setRows[0].id.key, "integration:web", "web is the first live member that is not in main")
        XCTAssertEqual(setRows[0].set?.next?.repo, "web")
        XCTAssertEqual(rows.count, 2, "the set and the single-repo integration")
        XCTAssertEqual(badgeCount([h.host]), 2)
    }

    func testAProjectWithoutSetsIsUnchanged() throws {
        var h = ReducerHarness()
        h.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:1", item: StoreWire.waitingItem("integration:1")),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:2", item: StoreWire.waitingItem("integration:2")),
            try StoreWire.end("proj-a"))
        let rows = inbox([h.host]).flatMap(\.rows)
        XCTAssertEqual(rows.map(\.id.key), ["integration:1", "integration:2"])
        XCTAssertTrue(rows.allSatisfy { $0.set == nil })
    }

    func testASetWithNoNextMemberKeepsItsFirstRow() throws {
        // Nothing waits for a live member of the set that is not in main when all of them are: the first stays.
        let confirmed = (0..<2).map { memberJSON("r\($0)", $0, "confirmed") }.joined(separator: ",")
        func row(_ repo: String, _ position: Int) -> String {
            "{\"key\":\"integration:\(repo)\",\"requestId\":\"i\(repo)\",\"kind\":\"integration\",\"since\":\"2026-10-01T09:00:00.000Z\","
                + "\"summary\":\"merged b\",\"contentHash\":\"h\",\"needsTerminal\":false,\"set\":{\"setId\":\"s\","
                + "\"state\":\"merged\",\"repo\":\"\(repo)\",\"position\":\(position),\"members\":[\(confirmed)]}}"
        }
        var h = ReducerHarness()
        h.apply(
            try StoreWire.hello(), try StoreWire.project("proj-a"),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:r1", item: row("r1", 1)),
            try StoreWire.snapshot("proj-a", "waiting", key: "integration:r0", item: row("r0", 0)),
            try StoreWire.end("proj-a"))
        XCTAssertEqual(inbox([h.host]).flatMap(\.rows).map(\.id.key), ["integration:r0"])
    }

    func testRowsWhoseSnapshotsDisagreeKeepTheSameRowWhicheverIsReadFirst() throws {
        // web knows api is in main; worker's older snapshot still has api built. The most advanced snapshot decides.
        func row(_ repo: String, _ position: Int, _ states: [String]) -> String {
            let members = zip(["api", "web", "worker"], states).enumerated().map { memberJSON($1.0, $0, $1.1) }
            return "{\"key\":\"integration:\(repo)\",\"requestId\":\"i\(repo)\",\"kind\":\"integration\","
                + "\"since\":\"2026-10-01T09:00:00.000Z\",\"summary\":\"merged b\",\"contentHash\":\"h\","
                + "\"needsTerminal\":false,\"set\":{\"setId\":\"s\",\"state\":\"merged\",\"repo\":\"\(repo)\","
                + "\"position\":\(position),\"members\":[\(members.joined(separator: ","))]}}"
        }
        let web = row("web", 1, ["confirmed", "merged", "merged"])
        let worker = row("worker", 2, ["merged", "merged", "merged"])
        var keys: [[String]] = []
        for order in [[("web", web), ("worker", worker)], [("worker", worker), ("web", web)]] {
            var h = ReducerHarness()
            h.apply(try StoreWire.hello(), try StoreWire.project("proj-a"))
            for (repo, item) in order {
                h.apply(try StoreWire.snapshot("proj-a", "waiting", key: "integration:\(repo)", item: item))
            }
            h.apply(try StoreWire.end("proj-a"))
            keys.append(inbox([h.host]).flatMap(\.rows).map(\.id.key))
        }
        XCTAssertEqual(keys[0], ["integration:web"])
        XCTAssertEqual(keys[0], keys[1])
    }

    func testTheSetSurvivesAnEffectThatDoesNotMentionIt() throws {
        var h = try harness([("web", 1, "2026-10-01T09:10:00.000Z")], extra: false)
        h.apply(.event(projectId: "proj-a", try StoreWire.event(
            "integration.finished", entity: ("integration", "web"),
            effects: [("waiting", "upsert", "integration:web", "{\"summary\":\"merged b2\"}")])))
        let stored = try XCTUnwrap(h.host.projects["proj-a"]?.waiting["integration:web"])
        XCTAssertEqual(stored.set?.setId, "set-4")
        XCTAssertEqual(stored.set?.members.count, 3)
    }
}
