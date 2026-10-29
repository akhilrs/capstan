import XCTest

final class ShParserTests: XCTestCase {
    func testWords() throws {
        XCTAssertEqual(try ShParser.split("cstan events --cursor c1.abc"), ["cstan", "events", "--cursor", "c1.abc"])
        XCTAssertEqual(try ShParser.split("  a \t b\n"), ["a", "b"])
        XCTAssertEqual(try ShParser.split(""), [])
    }

    func testQuotes() throws {
        XCTAssertEqual(try ShParser.split("a 'b c' \"d e\""), ["a", "b c", "d e"])
        XCTAssertEqual(try ShParser.split("x'y z'w"), ["xy zw"])
        XCTAssertEqual(try ShParser.split("''"), [""])
        XCTAssertEqual(try ShParser.split("'a\\b'"), ["a\\b"])
        XCTAssertEqual(try ShParser.split("\"a\\\"b\\\\c\\d\""), ["a\"b\\c\\d"])
        XCTAssertEqual(try ShParser.split("a\\ b"), ["a b"])
        XCTAssertEqual(try ShParser.split("'it'\\''s'"), ["it's"])
    }

    func testErrors() {
        XCTAssertThrowsError(try ShParser.split("'open"))
        XCTAssertThrowsError(try ShParser.split("\"open"))
        XCTAssertThrowsError(try ShParser.split("trail\\"))
    }
}
