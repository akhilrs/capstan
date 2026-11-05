import Foundation
import XCTest
@testable import CapstanKit

/// The `pm` member of the run item (document section 5): read leniently, kept through a merge and a saved file.
final class PmCardTests: XCTestCase {
    /// The run item of the document's example.
    private let documented = #"""
    {"requestId":"run","state":"active","paused":false,"contentHash":"b47b","pm":{"agentId":"pm-1","waitingOn":[{"kind":"report","count":2,"since":"2026-10-01T09:00:00.000Z"},{"kind":"review","count":1,"since":"2026-10-01T09:20:00.000Z"},{"kind":"user","count":3,"since":"2026-10-01T09:10:00.000Z"}],"fromUser":{"messageId":"msg-12","client":"phone","createdAt":"2026-10-01T09:25:00.000Z","state":"sent","stateSince":"2026-10-01T09:25:04.000Z","delivery":"delivered","body":{"text":"Which database should the tests use?","textVerified":true,"truncated":false}},"stuck":{"episodeId":"stuck-3f0c","reason":"unread_mail","since":"2026-10-01T09:20:00.000Z","alert":false}}}
    """#

    private func json(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    func testDocumentedRunItemReadsThePMCard() throws {
        let item = try RunItem(json: try json(documented))
        let pm = try XCTUnwrap(item.pm)
        XCTAssertEqual(pm.agentId, "pm-1")
        XCTAssertEqual(pm.waitingOn.map(\.kind), ["report", "review", "user"])
        XCTAssertEqual(pm.waitingOn.map(\.count), [2, 1, 3])
        XCTAssertEqual(pm.fromUser?.delivery, "delivered")
        XCTAssertEqual(pm.fromUser?.body?.raw, "Which database should the tests use?")
        XCTAssertEqual(pm.stuck?.reason, "unread_mail")
        XCTAssertEqual(pm.stuck?.alert, false)
    }

    func testNullOrAbsentPMIsNil() throws {
        let base = #"{"requestId":"run","state":"active","paused":false,"contentHash":"h""#
        XCTAssertNil(try RunItem(json: try json(base + "}")).pm)
        XCTAssertNil(try RunItem(json: try json(base + #","pm":null}"#)).pm)
    }

    func testAMalformedPMDropsOnlyThePMCard() throws {
        let text = #"{"requestId":"run","state":"active","paused":false,"contentHash":"h","pm":{"waitingOn":7}}"#
        let item = try RunItem(json: try json(text))
        XCTAssertNil(item.pm)
        XCTAssertEqual(item.contentHash, "h")
    }

    func testUnvouchedBodyAndZeroCountsAreDropped() throws {
        let text = #"""
        {"agentId":"pm-1","waitingOn":[{"kind":"verify","count":0},{"kind":"later_kind","count":4}],"fromUser":{"messageId":"m","createdAt":"2026-10-01T09:25:00Z","delivery":"queued","body":null},"stuck":null}
        """#
        let pm = try XCTUnwrap(PmCard(wire: try json(text)))
        XCTAssertEqual(pm.waitingOn.map(\.kind), ["later_kind"])
        XCTAssertNotNil(pm.fromUser)
        XCTAssertNil(pm.fromUser?.body)
        XCTAssertNil(pm.stuck)
    }

    func testStoredRunKeepsThePMCardThroughJSONAndASavedFile() throws {
        let stored = StoredRun(try RunItem(json: try json(documented)))
        XCTAssertEqual(try RunItem(json: stored.json).pm, stored.pm)
        let saved = try JSONDecoder().decode(StoredRun.self, from: try JSONEncoder().encode(stored))
        XCTAssertEqual(saved, stored)
    }

    func testAFileFromBeforeThePMCardStillReads() throws {
        let old = #"{"requestId":"run","state":"active","paused":true,"contentHash":"h"}"#
        let saved = try JSONDecoder().decode(StoredRun.self, from: Data(old.utf8))
        XCTAssertNil(saved.pm)
        XCTAssertTrue(saved.paused)
    }
}
