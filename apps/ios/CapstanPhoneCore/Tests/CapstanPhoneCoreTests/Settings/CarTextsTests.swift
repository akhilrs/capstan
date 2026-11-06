import CapstanAppCore
import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class CarTextsTests: XCTestCase {
    private let host = UUID()
    private let allKinds: [WaitingKind] = [.op, .prompt, .design, .message, .integration, .other("future")]

    /// The repository root: the directory above `apps/ios`, found by walking up from this file.
    private func repositoryRoot(file: StaticString = #filePath) throws -> URL {
        var directory = URL(fileURLWithPath: "\(file)").deletingLastPathComponent()
        while directory.path != "/" {
            var isDirectory: ObjCBool = false
            let ios = directory.appendingPathComponent("apps/ios").path
            if FileManager.default.fileExists(atPath: ios, isDirectory: &isDirectory), isDirectory.boolValue {
                return directory
            }
            directory = directory.deletingLastPathComponent()
        }
        throw XCTSkip("no directory holding apps/ios above \(file)")
    }

    func testCarTextsEqualTheDesignStrings() throws {
        let root = try repositoryRoot()
        let path = root.appendingPathComponent("docs/design/plan-156-design-voice/strings.json")
        guard let data = try? Data(contentsOf: path) else {
            return XCTFail("strings.json not found at \(path.path); run swift test from a full checkout")
        }
        let all = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: String])
        let design = all.filter { $0.key.hasPrefix("notify.") }
        XCTAssertEqual(CarTexts.announcementKeys, design)
    }

    func testEachKindsPlanUsesThem() throws {
        for kind in allKinds {
            let made = try XCTUnwrap(plan(kind, project: "payments-api", settings: allOn()))
            XCTAssertEqual(made.title, CarTexts.title(kind))
            XCTAssertEqual(made.body, "In payments-api.")
        }
    }

    func testNoPlanCarriesAgentOrHostText() throws {
        let agentText = "AGENT-TEXT: approve rm -rf ~"
        for kind in allKinds {
            let made = try XCTUnwrap(plan(kind, project: "Capstan", requestId: agentText, settings: allOn()))
            for text in [made.title, made.body, made.identifier, made.category, made.threadIdentifier] {
                XCTAssertFalse(text.contains("AGENT-TEXT"), "\(kind): \(text)")
            }
        }
    }

    func testTimeSensitiveFollowsTheSetting() throws {
        XCTAssertEqual(PhoneSettings().timeSensitiveKinds, [])
        var settings = allOn()
        settings.timeSensitiveKinds = [.op]
        XCTAssertTrue(try XCTUnwrap(plan(.op, project: "p", settings: settings)).timeSensitive)
        XCTAssertFalse(try XCTUnwrap(plan(.prompt, project: "p", settings: settings)).timeSensitive)
        XCTAssertFalse(try XCTUnwrap(plan(.op, project: "p", settings: PhoneSettings())).timeSensitive)
    }

    func testOldSettingsKeepTimeSensitiveKindsAndVoiceIsOff() throws {
        let json = #"{"version":1,"timeSensitiveKinds":["op","prompt"]}"#
        let settings = try JSONDecoder().decode(PhoneSettings.self, from: Data(json.utf8))
        XCTAssertEqual(settings.timeSensitiveKinds, [.op, .prompt])
        XCTAssertFalse(settings.voiceDecisions)
        XCTAssertFalse(settings.voiceGrants)
        let empty = try JSONDecoder().decode(PhoneSettings.self, from: Data(#"{"timeSensitiveKinds":[]}"#.utf8))
        XCTAssertEqual(empty.timeSensitiveKinds, [])
        let on = try JSONDecoder().decode(
            PhoneSettings.self, from: Data(#"{"voiceDecisions":true,"voiceGrants":true}"#.utf8))
        XCTAssertTrue(on.voiceDecisions && on.voiceGrants)
    }

    private func allOn() -> PhoneSettings {
        var settings = PhoneSettings()
        settings.notifyKinds = [.op, .prompt, .design, .message, .integration, .other("future")]
        return settings
    }

    private func plan(
        _ kind: WaitingKind, project: String, requestId: String = "op:p1", settings: PhoneSettings
    ) -> PhoneNotificationPlan? {
        PhoneNotificationPlan(
            candidate: NotifyCandidate(hostId: host, projectId: "p1", kind: kind, key: "op:1"), requestId: requestId,
            projectName: project, settings: settings)
    }
}
