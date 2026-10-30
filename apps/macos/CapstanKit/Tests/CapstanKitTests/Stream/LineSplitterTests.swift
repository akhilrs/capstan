import XCTest
import CapstanKit

/// The public splitter, as an SSH client outside the Kit uses it.
final class LineSplitterTests: XCTestCase {
    func testChunksSplitIntoLines() throws {
        var splitter = LineSplitter()
        XCTAssertEqual(try splitter.feed(Data("one\ntw".utf8)), [Data("one".utf8)])
        XCTAssertEqual(try splitter.feed(Data("o\n\nthree\n".utf8)), [Data("two".utf8), Data(), Data("three".utf8)])
        XCTAssertEqual(try splitter.feed(Data()), [])
    }

    func testLongestLinePasses() throws {
        var splitter = LineSplitter()
        let line = Data(repeating: 0x61, count: LineSplitter.maxLineBytes)
        XCTAssertEqual(try splitter.feed(line + Data("\n".utf8)), [line])
    }

    func testOneByteMoreThrows() {
        var splitter = LineSplitter()
        let line = Data(repeating: 0x61, count: LineSplitter.maxLineBytes + 1)
        XCTAssertThrowsError(try splitter.feed(line + Data("\n".utf8))) { error in
            XCTAssertEqual(error as? LineDecodeError, .tooLong)
        }
    }

    func testOneByteMoreWithoutNewlineThrows() {
        var splitter = LineSplitter()
        let line = Data(repeating: 0x61, count: LineSplitter.maxLineBytes + 1)
        XCTAssertThrowsError(try splitter.feed(line)) { error in
            XCTAssertEqual(error as? LineDecodeError, .tooLong)
        }
    }

    func testUnterminatedTailIsNotReturned() throws {
        var splitter = LineSplitter()
        XCTAssertEqual(try splitter.feed(Data("done\npartial".utf8)), [Data("done".utf8)])
        XCTAssertEqual(try splitter.feed(Data()), [])
    }
}
