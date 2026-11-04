import CapstanKit
import Foundation
import XCTest
@testable import CapstanAppCore

final class SentMessagesTests: XCTestCase {
    private let host = UUID()

    func testThreadsAreKeptPerProject() {
        var log = SentMessageLog()
        log.append(SentMessage(hostId: host, projectId: "a", text: "one", sentAt: Date(timeIntervalSince1970: 1)))
        log.append(SentMessage(hostId: host, projectId: "b", text: "two", sentAt: Date(timeIntervalSince1970: 2)))
        XCTAssertEqual(log.thread(hostId: host, projectId: "a").map(\.text), ["one"])
        XCTAssertEqual(log.thread(hostId: UUID(), projectId: "a").count, 0)
    }

    /// The host's limit for a message to the PM is the feed text cap (4096 bytes) once kernel fix PM-410
    /// (fix/PM-410-req-pm-message-cap) is in; before it the host cut such a message at 1024 bytes.
    func testTheTextCapIsUnderTheProtocolTextCap() {
        XCTAssertEqual(SentMessageLog.textCap, 4000)
        XCTAssertLessThan(SentMessageLog.textCap, ProtocolLimits.textCap)
        XCTAssertEqual(ProtocolLimits.textCap, 4096)
    }

    func testStatesAndTheCap() {
        var log = SentMessageLog()
        let first = SentMessage(hostId: host, projectId: "a", text: "x", sentAt: Date())
        log.append(first)
        log.update(first.id, state: .sent(messageId: "m-1"))
        XCTAssertEqual(log.messages.first?.state, .sent(messageId: "m-1"))
        for index in 0..<SentMessageLog.perProjectCap {
            log.append(SentMessage(hostId: host, projectId: "a", text: String(index), sentAt: Date()))
        }
        XCTAssertEqual(log.thread(hostId: host, projectId: "a").count, SentMessageLog.perProjectCap)
        XCTAssertFalse(log.messages.contains { $0.id == first.id }, "the oldest goes first")
        log.removeHost(host)
        XCTAssertTrue(log.messages.isEmpty)
    }

    func testRoundTripAndAnInterruptedSendReadsAsNotSent() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = SentMessageStore(directory: directory)
        var log = SentMessageLog()
        let sending = SentMessage(hostId: host, projectId: "a", text: "on its way", sentAt: Date(timeIntervalSince1970: 5))
        let sent = SentMessage(
            hostId: host, projectId: "a", text: "done", sentAt: Date(timeIntervalSince1970: 6), state: .sent(messageId: "m"))
        log.append(sending)
        log.append(sent)
        try store.save(log)
        let back = store.load()
        XCTAssertEqual(back.messages.count, 2)
        XCTAssertEqual(back.messages[1].state, .sent(messageId: "m"))
        guard case .notSent = back.messages[0].state else { return XCTFail("an interrupted send is not left sending") }
        XCTAssertEqual(SentMessageStore(directory: directory.appendingPathComponent("none")).load().messages.count, 0)
    }
}

final class SentMessageDeliveryTests: XCTestCase {
    private let host = UUID()

    private func log(_ id: String) -> SentMessageLog {
        var log = SentMessageLog()
        log.append(SentMessage(hostId: host, projectId: "p", text: "ship it", sentAt: Date(timeIntervalSince1970: 0),
                               state: .sent(messageId: id)))
        return log
    }

    func testTheHostStatesMoveAMessageFromQueuedToRead() {
        var log = log("m1")
        log.noteDelivery(messageId: "m1", hostState: "queued", at: Date(timeIntervalSince1970: 10))
        XCTAssertEqual(log.messages[0].delivery, .queued(since: Date(timeIntervalSince1970: 10)))
        log.noteDelivery(messageId: "m1", hostState: "deferred", at: Date(timeIntervalSince1970: 20))
        XCTAssertEqual(log.messages[0].delivery, .queued(since: Date(timeIntervalSince1970: 10)), "keeps the first time")
        XCTAssertTrue(log.messages[0].stuck(now: Date(timeIntervalSince1970: 200)))
        log.noteDelivery(messageId: "m1", hostState: "sent", at: Date(timeIntervalSince1970: 30))
        log.noteDelivery(messageId: "m1", hostState: "acked", at: Date(timeIntervalSince1970: 40))
        XCTAssertEqual(log.messages[0].delivery, .read(at: Date(timeIntervalSince1970: 40)))
        log.noteDelivery(messageId: "m1", hostState: "sent", at: Date(timeIntervalSince1970: 50))
        XCTAssertEqual(log.messages[0].delivery, .read(at: Date(timeIntervalSince1970: 40)), "never goes back")
    }

    func testARestartMovesTheMessageToItsNewId() {
        var log = log("m1")
        log.noteDelivery(messageId: "m1", hostState: "cancelled", at: Date(timeIntervalSince1970: 10))
        log.noteDelivery(messageId: "m2", hostState: "queued", at: Date(timeIntervalSince1970: 10), requeuedFrom: "m1")
        XCTAssertEqual(log.messages[0].messageId, "m2")
        XCTAssertEqual(log.messages[0].delivery, .queued(since: Date(timeIntervalSince1970: 10)))
        log.noteDelivery(messageId: "other", hostState: "acked", at: Date(timeIntervalSince1970: 20))
        XCTAssertEqual(log.messages[0].delivery, .queued(since: Date(timeIntervalSince1970: 10)))
    }
}

final class SentMessageEventsTests: XCTestCase {
    private let host = UUID()

    private func log(_ id: String) -> SentMessageLog {
        var log = SentMessageLog()
        log.append(SentMessage(hostId: host, projectId: "p", text: "ship it", sentAt: Date(timeIntervalSince1970: 0),
                               state: .sent(messageId: id)))
        return log
    }

    private func event(
        _ type: String = "message.state", id: String?, state: String?, requeuedFrom: String? = nil,
        ts: String = "2026-10-01T10:00:00Z"
    ) throws -> FeedEvent {
        var members = ["\"effects\":[]"]
        if let id { members.append("\"messageId\":\"\(id)\"") }
        if let state { members.append("\"state\":\"\(state)\"") }
        if let requeuedFrom { members.append("\"requeuedFrom\":\"\(requeuedFrom)\"") }
        let json = "{\"id\":\"e1\",\"seq\":1,\"k\":1,\"ts\":\"\(ts)\",\"type\":\"\(type)\","
            + "\"entity\":{\"type\":\"message\",\"id\":\"m\"},\"body\":{" + members.joined(separator: ",") + "}}"
        return try JSONDecoder().decode(FeedEvent.self, from: Data(json.utf8))
    }

    private let at = ISO8601DateFormatter().date(from: "2026-10-01T10:00:00Z")!

    func testQueuedDeliveredAndReadEventsMoveTheMessage() throws {
        var log = log("m1")
        XCTAssertTrue(log.noteEvents([try event(id: "m1", state: "queued")]))
        XCTAssertEqual(log.messages[0].delivery, .queued(since: at))
        XCTAssertTrue(log.noteEvents([try event("message.action_needed", id: "m1", state: "sent")]))
        XCTAssertEqual(log.messages[0].delivery, .delivered(at: at))
        XCTAssertTrue(log.noteEvents([try event(id: "m1", state: "acked")]))
        XCTAssertEqual(log.messages[0].delivery, .read(at: at))
    }

    func testAnEventThatChangesNothingReturnsFalse() throws {
        var log = log("m1")
        XCTAssertFalse(log.noteEvents([try event(id: "unknown", state: "acked")]), "unknown id")
        XCTAssertFalse(log.noteEvents([try event("agent.state", id: "m1", state: "acked")]), "other event type")
        XCTAssertFalse(log.noteEvents([try event(id: nil, state: "acked")]), "no message id")
        XCTAssertFalse(log.noteEvents([try event(id: "m1", state: nil)]), "no state")
        XCTAssertNil(log.messages[0].delivery)
        _ = log.noteEvents([try event(id: "m1", state: "acked")])
        XCTAssertFalse(log.noteEvents([try event(id: "m1", state: "sent")]), "an older state never replaces a newer one")
    }

    func testRequeuedFromFollowsTheMessageToItsNewId() throws {
        var log = log("m1")
        XCTAssertTrue(log.noteEvents([try event(id: "m2", state: "queued", requeuedFrom: "m1")]))
        XCTAssertEqual(log.messages[0].messageId, "m2")
        XCTAssertEqual(log.messages[0].delivery, .queued(since: at))
    }
}
