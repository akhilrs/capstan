import Foundation
import XCTest
@testable import CapstanKit

/// The `messages` section: messages from the agents to the person (`cstan send @user`).
final class MessagesSectionTests: XCTestCase {
    private func messageItem(_ sequence: Int, state: String = "open", body: String = "Ship plan-12 today? 1. Yes 2. No")
        -> String {
        "{\"key\":\"\(sequence)\",\"messageId\":\"msg-\(sequence)\",\"sequence\":\(sequence),\"senderAgentId\":\"pm-1\","
            + "\"class\":\"\(state == "info" ? "info" : "action")\",\"state\":\"\(state)\",\"body\":\"\(body)\","
            + "\"truncated\":false,\"createdAt\":\"\(StoreWire.since)\",\"answeredAt\":null,"
            + "\"requestId\":\"message:msg-\(sequence)\"}"
    }

    func testAMessageItemDecodesWithItsClassStateAndPlainBody() throws {
        let item = try MessageItem(json: try decodeJSON(Data(messageItem(3).utf8)))
        XCTAssertEqual(item.key, "3")
        XCTAssertEqual(item.sequence, 3)
        XCTAssertEqual(item.senderAgentId, "pm-1")
        XCTAssertEqual(item.messageClass, "action")
        XCTAssertEqual(item.state, "open")
        XCTAssertEqual(item.body.raw, "Ship plan-12 today? 1. Yes 2. No")
        XCTAssertFalse(item.body.truncated)
        XCTAssertNil(item.answeredAt)
        XCTAssertEqual(item.requestId, "message:msg-3")
    }

    func testTheSnapshotKeepsMessagesAtItsEndAndAnInvalidateAsksForARead() throws {
        var harness = ReducerHarness()
        harness.apply(try StoreWire.hello(projects: 1), try StoreWire.project("proj-a"))
        harness.apply(
            try StoreWire.snapshot("proj-a", "messages", key: "1", item: messageItem(1, state: "info", body: "Merged.")),
            try StoreWire.snapshot("proj-a", "messages", key: "2", item: messageItem(2)))
        XCTAssertNil(harness.alpha?.messages, "nothing is kept before snapshotEnd")
        harness.apply(try StoreWire.end("proj-a"))
        let messages = try XCTUnwrap(harness.alpha?.messages)
        XCTAssertEqual(messages.keys.sorted(), ["1", "2"])
        XCTAssertFalse(try XCTUnwrap(messages["1"]).isOpenQuestion)
        XCTAssertTrue(try XCTUnwrap(messages["2"]).isOpenQuestion)

        let event = try StoreWire.event(
            "snapshot.invalidated", entity: ("user_message", "msg-2"),
            effects: [("messages", "invalidate", "2", "{}"), ("waiting", "invalidate", "message:2", "{}")])
        let output = harness.apply(.event(projectId: "proj-a", event))
        XCTAssertEqual(output.refresh, ["proj-a"])
        XCTAssertNotNil(harness.alpha?.messages?["2"], "an invalidate keeps the item until the read")

        let removed = try StoreWire.event(
            "snapshot.invalidated", entity: ("user_message", "msg-1"), seq: 31,
            effects: [("messages", "remove", "1", "{}")])
        harness.apply(.event(projectId: "proj-a", removed))
        XCTAssertNil(harness.alpha?.messages?["1"])
    }

    func testAQuestionsChoicesDecodeAndAnOlderHostHasNone() throws {
        let withOptions = messageItem(4).replacingOccurrences(
            of: "}", with: ",\"options\":[\"Close out plan-8 (Recommended)\",\"Start the follow-ups\"]}")
        let item = try MessageItem(json: try decodeJSON(Data(withOptions.utf8)))
        XCTAssertEqual(item.options, ["Close out plan-8 (Recommended)", "Start the follow-ups"])
        let older = try MessageItem(json: try decodeJSON(Data(messageItem(5).utf8)))
        XCTAssertEqual(older.options, [])
    }
}
