import Foundation
import XCTest
@testable import CapstanPhoneCore

final class SentMessagesTests: XCTestCase {
    private let host = UUID()

    func testThreadsAreKeptPerProject() {
        var log = SentMessageLog()
        log.append(SentMessage(hostId: host, projectId: "a", text: "one", sentAt: Date(timeIntervalSince1970: 1)))
        log.append(SentMessage(hostId: host, projectId: "b", text: "two", sentAt: Date(timeIntervalSince1970: 2)))
        XCTAssertEqual(log.thread(hostId: host, projectId: "a").map(\.text), ["one"])
        XCTAssertEqual(log.thread(hostId: UUID(), projectId: "a").count, 0)
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
