import XCTest
@testable import CapstanKit

final class NegativeTests: XCTestCase {
    private let epoch = "daeca3b2245f3519"
    private let anchor = "0f1e2d3c4b5a6978"

    private func cursorText(_ json: String) -> String {
        Cursor.prefix + CursorCodec.base64URLEncode(Data(json.utf8))
    }

    private func entry(_ fields: String) -> String {
        "{\"p\":{\"a\":{\(fields)}}}"
    }

    private func assertRejected(_ text: String, _ message: String, line: UInt = #line) {
        guard let cursor = Cursor(text: text) else { return }
        XCTAssertThrowsError(try cursor.decoded(), message, line: line)
    }

    private func heartbeat(totalBytes: Int) -> Data {
        let head = "{\"v\":1,\"kind\":\"heartbeat\",\"ts\":\""
        let tail = "\"}"
        let pad = totalBytes - head.utf8.count - tail.utf8.count
        return Data((head + String(repeating: "a", count: pad) + tail).utf8)
    }

    // MARK: lines

    func testVersionTwo() {
        let line = Data("{\"v\":2,\"kind\":\"heartbeat\",\"ts\":\"x\"}".utf8)
        XCTAssertThrowsError(try LineDecoder().decode(line)) {
            XCTAssertEqual($0 as? LineDecodeError, .unsupportedVersion(2))
        }
    }

    func testLineLength() throws {
        XCTAssertEqual(LineDecoder.maxLineBytes, 65536)
        let fits = heartbeat(totalBytes: 65535)
        XCTAssertEqual(fits.count, 65535)
        XCTAssertEqual(try LineDecoder().decode(fits).heartbeatTime?.utf8.count, 65535 - 32 - 2)
        let over = heartbeat(totalBytes: 65536)
        XCTAssertThrowsError(try LineDecoder().decode(over)) {
            XCTAssertEqual($0 as? LineDecodeError, .tooLong)
        }
    }

    func testUnknownKind() throws {
        let line = Data("{\"v\":1,\"kind\":\"later\",\"x\":1}".utf8)
        XCTAssertEqual(try LineDecoder().decode(line), .unknownKind("later"))
    }

    func testUnknownFieldIgnored() throws {
        let line = Data("{\"v\":1,\"kind\":\"hello\",\"version\":\"1\",\"host\":\"h\",\"projects\":0,\"more\":{\"a\":[1]}}".utf8)
        XCTAssertEqual(try LineDecoder().decode(line), .hello(Hello(version: "1", host: "h", projects: 0)))
    }

    func testMalformedLines() {
        for text in ["", "[]", "{}", "{\"v\":1}", "{\"v\":1,\"kind\":\"hello\"}", "not json"] {
            XCTAssertThrowsError(try LineDecoder().decode(Data(text.utf8)), text) {
                XCTAssertNotEqual($0 as? LineDecodeError, .tooLong)
            }
        }
    }

    func testUnknownSectionKeepsItem() throws {
        let line = Data("{\"v\":1,\"kind\":\"snapshot\",\"projectId\":\"p\",\"section\":\"later\",\"key\":\"k\",\"item\":{\"a\":1}}".utf8)
        guard case .snapshot(let snapshot) = try LineDecoder().decode(line) else { return XCTFail("snapshot") }
        XCTAssertEqual(snapshot.item, .unknown(section: "later", .object(["a": .int(1)])))
    }

    // MARK: cursors

    func testCanonicalCursorDecodes() throws {
        let text = cursorText(entry("\"e\":\"\(epoch)\",\"h\":\"\(anchor)\",\"r\":\"/x\",\"s\":5"))
        let payload = try XCTUnwrap(Cursor(text: text)).decoded()
        XCTAssertEqual(payload.p["a"], CursorEntry(e: epoch, s: 5, h: anchor, r: "/x"))
    }

    func testRejectedCursors() throws {
        let good = cursorText(entry("\"e\":\"\(epoch)\",\"h\":\"\(anchor)\",\"r\":\"/x\",\"s\":5"))
        XCTAssertNotNil(Cursor(text: good))
        XCTAssertNil(Cursor(text: good + "="), "padding")
        XCTAssertNil(Cursor(text: "c2." + good.dropFirst(3)), "prefix")
        XCTAssertNil(Cursor(text: "eyJw"), "no prefix")
        XCTAssertNil(Cursor(text: "c1."), "empty")
        XCTAssertNil(Cursor(text: "c1.a+b"), "standard alphabet")
        let e = "\"e\":\"\(epoch)\""
        let h = "\"h\":\"\(anchor)\""
        assertRejected(cursorText(entry("\(h),\(e),\"r\":\"/x\",\"s\":5")), "reordered entry keys")
        assertRejected(cursorText(entry("\(e),\(h),\"r\":\"\\/x\",\"s\":5")), "escaped slash")
        assertRejected(cursorText(entry("\(e),\"k\":0,\(h),\"r\":\"/x\",\"s\":5")), "k 0")
        assertRejected(cursorText(entry("\(e),\(h),\"r\":\"/x\",\"s\":-1")), "s -1")
        assertRejected(cursorText(entry("\"e\":\"daeca3b2245f351\",\(h),\"r\":\"/x\",\"s\":5")), "15 hex")
        assertRejected(cursorText(entry("\"e\":\"DAECA3B2245F3519\",\(h),\"r\":\"/x\",\"s\":5")), "uppercase hex")
        assertRejected(cursorText(entry("\(e),\(h),\"s\":5")), "missing r")
        assertRejected(cursorText(entry("\(e),\(h),\"r\":\"/x\",\"s\":5,\"z\":1")), "extra field")
        assertRejected(cursorText(entry("\(e),\(h),\"r\":\"/x\",\"s\":\"5\"")), "wrong type")
        assertRejected(cursorText(entry("\(e),\(h),\"r\":\"/x\",\"s\":9223372036854775808")), "s above max")
        assertRejected(cursorText("{\"p\":{},\"q\":1}"), "top-level q")
        assertRejected(cursorText("{\"q\":{}}"), "no p")
        assertRejected(cursorText("{\"p\":{\"a\":{\(e),\(h),\"r\":\"/x\",\"r\":\"/x\",\"s\":5}}}"), "duplicate key")
        assertRejected(cursorText("{\"p\":{\"a\":{\(e),\(h),\"r\":\"/x\",\"s\":5}}} "), "whitespace")
        assertRejected("c1.AAAA", "not json")
    }

    func testNonASCIIRootRoundTrips() throws {
        let json = "{\"p\":{\"a\":{\"e\":\"\(epoch)\",\"h\":\"\(anchor)\",\"r\":\"/srv/\u{E9}\\\"\\u0001x\",\"s\":3}}}"
        let text = cursorText(json)
        let payload = try XCTUnwrap(Cursor(text: text)).decoded()
        XCTAssertEqual(payload.p["a"]?.r, "/srv/\u{E9}\"\u{01}x")
        XCTAssertEqual(Cursor(payload: payload).text, text)
    }

    func testEscapes() throws {
        let root = "\u{08}\u{0C}\n\r\t\\\u{1F}\u{7F}"
        let payload = CursorPayload(p: ["a": CursorEntry(e: epoch, s: 0, h: "0000000000000000", r: root)])
        XCTAssertEqual(
            CursorCodec.json(payload),
            "{\"p\":{\"a\":{\"e\":\"\(epoch)\",\"h\":\"0000000000000000\",\"r\":\"\\b\\f\\n\\r\\t\\\\\\u001f\u{7F}\",\"s\":0}}}")
        XCTAssertEqual(try Cursor(payload: payload).decoded(), payload)
    }

    func testEntryFieldsRoundTrip() throws {
        let payload = CursorPayload(p: [
            "b": CursorEntry(e: epoch, s: 7, k: 1, h: anchor, r: "/b", d: 1_790_000_000),
            "a": CursorEntry(e: epoch, s: Int64.max, h: anchor, r: "/a"),
            "B": CursorEntry(e: epoch, s: 0, h: "0000000000000000", r: "/c"),
        ])
        let cursor = Cursor(payload: payload)
        XCTAssertEqual(try cursor.decoded(), payload)
        XCTAssertTrue(CursorCodec.json(payload).hasPrefix("{\"p\":{\"B\":"))
    }

    func testRemoving() throws {
        let payload = CursorPayload(p: [
            "a": CursorEntry(e: epoch, s: 1, h: anchor, r: "/a"),
            "b": CursorEntry(e: epoch, s: 2, h: anchor, r: "/b"),
        ])
        let cursor = Cursor(payload: payload)
        XCTAssertNil(try cursor.removing(["a", "b"]))
        XCTAssertNil(try cursor.removing(["a", "b", "c"]))
        let left = try XCTUnwrap(try cursor.removing(["a"]))
        XCTAssertEqual(try left.decoded().p.keys.sorted(), ["b"])
        XCTAssertEqual(try cursor.removing([]), cursor)
        XCTAssertThrowsError(try XCTUnwrap(Cursor(text: "c1.AAAA")).removing(["a"]))
    }

    // MARK: text

    func testDisplayPlain() {
        let text = UntrustedText(raw: "a\u{202E}b\nc\u{0}d\u{85}e\u{2028}f\u{FEFF}g\u{2066}h", verified: true, truncated: false)
        XCTAssertEqual(text.displayPlain, "a b c d e f g h")
        XCTAssertEqual(UntrustedText(raw: "plain \u{E9}", verified: true, truncated: false).displayPlain, "plain \u{E9}")
    }

    func testUnverifiedTextIsEmpty() {
        let text = UntrustedText(raw: "secret", verified: false, truncated: true)
        XCTAssertEqual(text.raw, "")
        XCTAssertEqual(text.displayPlain, "")
        XCTAssertTrue(text.truncated)
    }

    func testJSONValueMerging() {
        let base = JSONValue.object(["a": .int(1), "b": .string("x")])
        let merged = base.merging(fields: .object(["b": .null, "c": .bool(true)]))
        XCTAssertEqual(merged["a"], .int(1))
        XCTAssertEqual(merged["b"], .null)
        XCTAssertEqual(merged["c"], .bool(true))
        XCTAssertEqual(JSONValue.null.merging(fields: base), .null)
        XCTAssertEqual(JSONValue.array([.int(2)])[0], .int(2))
    }
}

private extension StreamLine {
    var heartbeatTime: String? {
        if case .heartbeat(let ts) = self { return ts }
        return nil
    }
}
