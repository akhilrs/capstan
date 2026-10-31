import CapstanAppCore
import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class SettingsTests: XCTestCase {
    func testDefaults() {
        let settings = PhoneSettings()
        XCTAssertTrue(settings.faceID)
        XCTAssertEqual(settings.pollMs, 2000)
        XCTAssertTrue(settings.heartbeatPulse)
        XCTAssertEqual(settings.notifyKinds, [.op, .prompt, .design, .message, .integration])
        XCTAssertEqual(settings.timeSensitiveKinds, [])
        XCTAssertFalse(settings.liveActivityAllowed)
        XCTAssertEqual(settings.sounds, SoundPreferences())
        XCTAssertEqual(settings.version, PhoneSettings.currentVersion)
    }

    func testPollValidation() {
        XCTAssertEqual(SettingsValidation.pollError(999), "At least 1000 ms")
        XCTAssertNil(SettingsValidation.pollError(1000))
        XCTAssertNil(SettingsValidation.pollError(2000))
        XCTAssertNil(SettingsValidation.pollError(HostLimits.maxPollMs))
        XCTAssertEqual(SettingsValidation.pollError(HostLimits.maxPollMs + 1), "At most \(HostLimits.maxPollMs) ms")
    }

    func testStepperStaysWithinTheLimits() {
        XCTAssertEqual(SettingsValidation.step(2000, up: true), 2500)
        XCTAssertEqual(SettingsValidation.step(2000, up: false), 1500)
        XCTAssertEqual(SettingsValidation.step(1000, up: false), 1000)
        XCTAssertEqual(SettingsValidation.step(1200, up: false), 1000)
        XCTAssertEqual(SettingsValidation.step(HostLimits.maxPollMs, up: true), HostLimits.maxPollMs)
        XCTAssertEqual(SettingsValidation.clampPoll(999), 1000)
    }

    func testClientLabel() {
        XCTAssertTrue(SettingsValidation.isClientLabel("iPhone"))
        XCTAssertTrue(SettingsValidation.isClientLabel("Akhil's iPhone".replacingOccurrences(of: "'", with: "")))
        XCTAssertFalse(SettingsValidation.isClientLabel(""))
        XCTAssertFalse(SettingsValidation.isClientLabel("Akhil's iPhone"))
        XCTAssertFalse(SettingsValidation.isClientLabel(String(repeating: "a", count: 65)))
    }

    func testJSONRoundTrip() throws {
        var settings = PhoneSettings()
        settings.faceID = false
        settings.pollMs = 5000
        settings.notifyKinds = [.prompt]
        settings.timeSensitiveKinds = [.prompt, .op]
        settings.liveActivityAllowed = true
        settings.sounds = SoundPreferences(defaultSound: .silent, perKind: ["prompt": .user("Bell.caf")])
        let data = try JSONEncoder().encode(settings)
        XCTAssertEqual(try JSONDecoder().decode(PhoneSettings.self, from: data), settings)
    }

    func testUnknownFutureFieldsAreIgnoredAndMissingOnesDefault() throws {
        let json = #"{"version":2,"faceID":false,"somethingNew":{"a":1},"pollMs":3000}"#
        let settings = try JSONDecoder().decode(PhoneSettings.self, from: Data(json.utf8))
        XCTAssertEqual(settings.version, 2)
        XCTAssertFalse(settings.faceID)
        XCTAssertEqual(settings.pollMs, 3000)
        XCTAssertTrue(settings.heartbeatPulse, "a missing key takes its default")
        XCTAssertEqual(settings.notifyKinds, PhoneSettings().notifyKinds)
    }

    func testStoreSaveLoadAndDamagedFile() throws {
        let directory = try makeTempDirectory(self)
        let store = PhoneSettingsStore(directory: directory)
        XCTAssertEqual(store.load(), PhoneSettings(), "a missing file gives the defaults")
        var settings = PhoneSettings()
        settings.pollMs = 7000
        try store.save(settings)
        XCTAssertEqual(store.load(), settings)
        try Data("{oops".utf8).write(to: directory.appendingPathComponent("settings.json"))
        XCTAssertEqual(store.load(), PhoneSettings(), "a damaged file gives the defaults")
    }
}
