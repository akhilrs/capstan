import XCTest
@testable import CapstanKit

final class ModelLabelTests: XCTestCase {
    func testTheSixSourcesMapToTheDesignLabels() {
        let table: [(String, String, String, String)] = [
            ("spawn", "spawn", "Chosen at spawn", "chosen at spawn"),
            ("task", "task", "Task override", "task override"),
            ("plan", "plan", "Plan override", "plan override"),
            ("role", "role", "Role setting", "role setting"),
            ("host", "host", "Host setting", "host setting"),
            ("defaults", "defaults", "Config defaults", "config defaults"),
        ]
        for (source, short, long, spoken) in table {
            let label = ModelLabel.label(source: source)
            XCTAssertEqual(label.short, short)
            XCTAssertEqual(label.long, long)
            XCTAssertEqual(label.spoken, spoken)
        }
    }

    func testAnUnknownEmptyOrAbsentSourceIsOther() {
        for source in ["fleet", "", "PLAN", nil] as [String?] {
            let label = ModelLabel.label(source: source)
            XCTAssertEqual(label.short, "other")
            XCTAssertEqual(label.long, "Other source")
            XCTAssertEqual(label.spoken, "other source")
        }
    }
}
