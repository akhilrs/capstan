import Foundation
import XCTest
@testable import CapstanPhoneCore

final class VoiceTextsTests: XCTestCase {
    /// The repository root: the directory above `apps/ios`, found by walking up from this file.
    private func stringsURL(file: StaticString = #filePath) -> URL? {
        var directory = URL(fileURLWithPath: "\(file)").deletingLastPathComponent()
        while directory.path != "/" {
            let candidate = directory.appendingPathComponent("docs/design/plan-156-design-voice/strings.json")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
            directory = directory.deletingLastPathComponent()
        }
        return nil
    }

    func testVoiceTextsEqualTheDesignStrings() throws {
        guard let path = stringsURL() else {
            return XCTFail("docs/design/plan-156-design-voice/strings.json not found above \(#filePath); run swift test from a full checkout")
        }
        let data = try Data(contentsOf: path)
        let design = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: String])
        XCTAssertEqual(VoiceTexts.all, design)
    }

    func testCapsMatchTheDesignNote() {
        XCTAssertEqual(VoiceTexts.LINE_CAP, 120)
        XCTAssertEqual(VoiceTexts.ITEM_CAP, 120)
    }

    func testFillReplacesEachPlaceholderOnce() {
        let line = VoiceTexts.fill("frame.command", ["command": "echo {command}"])
        XCTAssertEqual(line, "The command is: echo {command}. End of command.")
    }
}
