import CapstanKit
import Foundation
import XCTest

@testable import CapstanAppCore

final class ConversationsTests: XCTestCase {
    private let hostId = ShellFixtures.hostA
    private var config: HostConfig { HostConfig(id: hostId, displayName: "Build", destination: "build") }

    private func pm() throws -> StoredAgent {
        try ShellFixtures.decode(
            StoredAgent.self,
            "{\"key\":\"agent:pm-1\",\"agentId\":\"pm-1\",\"kind\":\"PM\",\"role\":\"pm\",\"generation\":1,"
                + "\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:pm-1\",\"contentHash\":\"h\"}")
    }

    private func question(_ sequence: Int64, state: String = "open", at: String) -> StoredMessage {
        StoredMessage(
            key: "m" + String(sequence), messageId: "id" + String(sequence), sequence: sequence, senderAgentId: "pm-1",
            messageClass: "action", state: state,
            body: UntrustedText(raw: "Question " + String(sequence), verified: true, truncated: false), createdAt: at,
            requestId: "message:" + String(sequence))
    }

    private func models(_ projects: [ProjectModel]) -> [UUID: HostModel] {
        [hostId: ShellFixtures.host(hostId, projects: projects)]
    }

    func testAProjectWithoutAPmOrMessagesIsLeftOut() throws {
        let rows = Conversations.make(
            hosts: [config], hostModels: models([try ShellFixtures.project("quiet")]), sent: SentMessageLog())
        XCTAssertTrue(rows.isEmpty)
    }

    func testAnActivePmMakesAConversationWithNoMessagesYet() throws {
        let project = try ShellFixtures.project("alpha", agents: [try pm()])
        let rows = Conversations.make(hosts: [config], hostModels: models([project]), sent: SentMessageLog())
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].pm, "pm-1")
        XCTAssertEqual(rows[0].last, "No messages yet")
        XCTAssertNil(rows[0].lastAt)
        XCTAssertEqual(rows[0].waiting, 0)
        XCTAssertEqual(rows[0].hostName, "Build")
    }

    func testNoPmStillListsAProjectWithAMessage() throws {
        var project = try ShellFixtures.project("alpha")
        project.messages = ["m1": question(1, at: "2026-10-01T10:00:00Z")]
        let rows = Conversations.make(hosts: [config], hostModels: models([project]), sent: SentMessageLog())
        XCTAssertEqual(rows.count, 1)
        XCTAssertNil(rows[0].pm)
        XCTAssertEqual(rows[0].last, "pm-1: Question 1")
    }

    func testWaitingCountsOnlyOpenQuestions() throws {
        var project = try ShellFixtures.project("alpha", agents: [try pm()])
        project.messages = [
            "m1": question(1, at: "2026-10-01T10:00:00Z"),
            "m2": question(2, at: "2026-10-01T10:05:00Z"),
            "m3": question(3, state: "answered", at: "2026-10-01T10:06:00Z"),
        ]
        let rows = Conversations.make(hosts: [config], hostModels: models([project]), sent: SentMessageLog())
        XCTAssertEqual(rows[0].waiting, 2)
        XCTAssertEqual(rows[0].last, "pm-1: Question 3", "the newest by sequence")
    }

    func testTheNewestLineIsTheLaterOfWhatCameAndWhatWasSent() throws {
        var project = try ShellFixtures.project("alpha", agents: [try pm()])
        project.messages = ["m1": question(1, at: "2026-10-01T10:00:00Z")]
        let received = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-10-01T10:00:00Z"))
        var log = SentMessageLog()
        log.append(SentMessage(
            hostId: hostId, projectId: "alpha", text: "on it", sentAt: received.addingTimeInterval(60),
            state: .sent(messageId: "x")))
        XCTAssertEqual(
            Conversations.make(hosts: [config], hostModels: models([project]), sent: log)[0].last, "You: on it · Sent")
        var older = SentMessageLog()
        older.append(SentMessage(hostId: hostId, projectId: "alpha", text: "old", sentAt: received.addingTimeInterval(-60)))
        XCTAssertEqual(
            Conversations.make(hosts: [config], hostModels: models([project]), sent: older)[0].last, "pm-1: Question 1")
    }

    func testNewestConversationComesFirst() throws {
        var a = try ShellFixtures.project("alpha", agents: [try pm()])
        a.messages = ["m1": question(1, at: "2026-10-01T10:00:00Z")]
        var b = try ShellFixtures.project("beta", agents: [try pm()])
        b.messages = ["m1": question(1, at: "2026-10-02T10:00:00Z")]
        let c = try ShellFixtures.project("gamma", agents: [try pm()])
        let rows = Conversations.make(hosts: [config], hostModels: models([a, c, b]), sent: SentMessageLog())
        XCTAssertEqual(rows.map(\.projectId), ["beta", "alpha", "gamma"])
    }

    func testMessageStateWords() {
        XCTAssertEqual(MessageStateText.short(.sending), "Sending")
        XCTAssertEqual(MessageStateText.short(.sent(messageId: "m")), "Sent")
        XCTAssertEqual(MessageStateText.short(.notSent(reason: "x", pendingId: nil)), "Not sent")
        XCTAssertEqual(MessageStateText.symbol(.read(at: Date())), "checkmark.circle.fill")
        let message = SentMessage(
            hostId: hostId, projectId: "p", text: "t", sentAt: Date(timeIntervalSince1970: 0), state: .sent(messageId: "m"),
            delivery: .queued(since: Date(timeIntervalSince1970: 0)))
        XCTAssertEqual(
            MessageStateText.delivery(message, now: Date(timeIntervalSince1970: 60)), "Queued: the PM gets it when it is free")
        XCTAssertEqual(
            MessageStateText.delivery(message, now: Date(timeIntervalSince1970: 300)),
            "Waiting for the PM for 5 min: it is busy, stuck at a question, or paused")
    }
}
