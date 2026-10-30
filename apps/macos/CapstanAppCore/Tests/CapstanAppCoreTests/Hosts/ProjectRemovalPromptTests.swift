import CapstanKit
import XCTest

@testable import CapstanAppCore

final class ProjectRemovalPromptTests: XCTestCase {
    func testOnlyAGoneOrSilentProjectIsOffered() {
        XCTAssertTrue(ProjectRemovalPrompt.isOffered(.gone))
        XCTAssertTrue(ProjectRemovalPrompt.isOffered(.silent))
        for live in [ProjectAvailability.ready, .noKey, .refused, .unsupported, .other("new")] {
            XCTAssertFalse(ProjectRemovalPrompt.isOffered(live), "\(live)")
        }
    }

    func testTheAlertNamesTheProjectAndTheRefusalsHaveWords() {
        XCTAssertEqual(ProjectRemovalPrompt.title(name: "api"), "Remove api from the list?")
        XCTAssertNil(ProjectRemovalPrompt.refusal(.removed))
        XCTAssertNotNil(ProjectRemovalPrompt.refusal(.refusedLive))
        XCTAssertNotNil(ProjectRemovalPrompt.refusal(.notFound))
    }
}
