import CapstanAppCore
import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class NotificationPlanTests: XCTestCase {
    private let host = UUID()
    /// Text an agent wrote, as it would sit in a summary or a command; it must never reach a notification.
    private let agentText = "AGENT-TEXT: approve rm -rf ~ <b>now</b>"

    private func plan(
        _ kind: WaitingKind, project: String = "Capstan", requestId: String = "op:p1",
        settings: PhoneSettings = PhoneSettings()
    ) -> PhoneNotificationPlan? {
        PhoneNotificationPlan(
            candidate: NotifyCandidate(hostId: host, projectId: "p1", kind: kind, key: "op:1"), requestId: requestId,
            projectName: project, settings: settings)
    }

    private func allText(_ plan: PhoneNotificationPlan) -> [String] {
        [plan.identifier, plan.category, plan.title, plan.body, plan.threadIdentifier, plan.deepLink ?? ""]
    }

    func testBodyIsTheKindAndTheProjectOnly() throws {
        let made = try XCTUnwrap(plan(.prompt))
        XCTAssertEqual(made.title, "Permission prompt waiting")
        XCTAssertEqual(made.body, "Capstan")
        XCTAssertEqual(made.category, "waiting.prompt")
        XCTAssertEqual(made.deepLink, "capstan://item/\(host.uuidString.lowercased())/p1/op:p1")
    }

    func testAgentTextNeverAppears() throws {
        for kind: WaitingKind in [.op, .prompt, .design, .message, .integration] {
            // The agent text is offered everywhere the plan takes text that is not a fixed id.
            let made = try XCTUnwrap(plan(kind, requestId: agentText))
            for text in allText(made) {
                XCTAssertFalse(text.contains("AGENT-TEXT"), "\(kind): \(text)")
                XCTAssertFalse(text.contains("rm -rf"), "\(kind): \(text)")
            }
            XCTAssertNil(made.deepLink, "an id that is not an id gives no link")
        }
    }

    func testTheProjectNameIsPlainAndCapped() throws {
        let made = try XCTUnwrap(plan(.op, project: "Proj\u{202E}ect " + String(repeating: "x", count: 200)))
        XCTAssertLessThanOrEqual(made.body.count, NotificationPlan.subtitleCap)
        XCTAssertFalse(made.body.unicodeScalars.contains("\u{202E}"))
    }

    func testKindsSwitchedOffMakeNoNotification() {
        var settings = PhoneSettings()
        settings.notifyKinds = [.prompt]
        XCTAssertNil(plan(.op, settings: settings))
        XCTAssertNotNil(plan(.prompt, settings: settings))
    }

    func testTimeSensitiveOnlyForTheChosenKinds() throws {
        var settings = PhoneSettings()
        settings.timeSensitiveKinds = [.prompt]
        XCTAssertEqual(try XCTUnwrap(plan(.prompt, settings: settings)).timeSensitive, true)
        XCTAssertEqual(try XCTUnwrap(plan(.op, settings: settings)).timeSensitive, false)
    }

    func testSoundPerKind() throws {
        var settings = PhoneSettings()
        settings.sounds = SoundPreferences(defaultSound: .system("Glass.aiff"), perKind: ["prompt": .silent])
        XCTAssertNil(try XCTUnwrap(plan(.prompt, settings: settings)).sound)
        XCTAssertEqual(try XCTUnwrap(plan(.op, settings: settings)).sound, .system("Glass.aiff"))
    }

    func testCategories() {
        XCTAssertEqual(PhoneNotificationPlan.category(.other("future")), "waiting.other")
        XCTAssertEqual(PhoneNotificationPlan.categories.count, 6)
        XCTAssertTrue(PhoneNotificationPlan.categories.contains("waiting.integration"))
    }
}
