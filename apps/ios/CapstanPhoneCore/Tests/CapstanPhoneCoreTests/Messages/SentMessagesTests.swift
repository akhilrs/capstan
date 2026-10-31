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
