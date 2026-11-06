import CapstanKit
import Foundation
import XCTest
@testable import CapstanAppCore

final class PmCardModelTests: XCTestCase {
    private var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? TimeZone.current
        return calendar
    }

    private func date(_ text: String) throws -> Date {
        try XCTUnwrap(ISO8601DateFormatter().date(from: text))
    }

    private func text(_ raw: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: false)
    }

    private func project(pm: PmCard?, activity: String? = "working", messages: [StoredMessage] = []) -> ProjectModel {
        let agent = StoredAgent(
            key: "pm-1", agentId: "pm-1", kind: "PM", role: "pm", generation: 1, state: "active", paused: false,
            task: nil, requestId: "agent:pm-1", contentHash: "h", branch: nil, activity: activity,
            activitySince: "2026-10-30T11:58:00Z", activitySeq: 1)
        var list: [String: StoredMessage] = [:]
        for message in messages { list[message.key] = message }
        var model = ProjectModel(
            projectId: "p1", root: "/work/p1", epoch: "e", head: 1, agents: ["pm-1": agent],
            run: StoredRun(requestId: "run", state: "active", paused: false, contentHash: "h", pm: pm),
            snapshotComplete: true)
        model.messages = list
        return model
    }

    private func make(
        _ project: ProjectModel, freshness: ProjectFreshness? = nil
    ) throws -> PmCardModel {
        let now = try date("2026-10-30T12:21:00Z")
        let fresh = freshness ?? ProjectFreshness(currentAsOf: now, lastKnownAt: now)
        return try XCTUnwrap(
            PmCardModel.make(project: project, projectName: "payments-api", freshness: fresh, now: now, calendar: calendar))
    }

    func testNoActivePMSaysSo() throws {
        let card = try make(project(pm: nil))
        XCTAssertNil(card.agentId)
        XCTAssertEqual(
            card.noPMText,
            "No PM is active on payments-api, so nobody routes work. The card fills in when a PM registers.")
    }

    func testHeaderWaitingRowsAndDelivery() throws {
        let pm = PmCard(
            agentId: "pm-1",
            waitingOn: [
                PmCard.Waiting(kind: "report", count: 3, since: "2026-10-30T10:11:00Z"),
                PmCard.Waiting(kind: "user", count: 1, since: "2026-10-30T12:09:00Z"),
                PmCard.Waiting(kind: "later_kind", count: 2, since: nil),
            ],
            fromUser: PmCard.FromUser(
                messageId: "m", createdAt: "2026-10-30T12:03:00Z", state: "sent", stateSince: "2026-10-30T12:05:00Z",
                delivery: "read", body: text("Use Postgres")))
        let card = try make(project(pm: pm))
        XCTAssertEqual(card.agentId, "pm-1")
        XCTAssertEqual(card.activityWord, "Working")
        XCTAssertEqual(card.sinceText, "since 11:58 · 23 min")
        XCTAssertEqual(card.freshness, .current)
        XCTAssertEqual(card.waitingOn.map(\.title), ["Worker reports", "You", "later_kind"])
        XCTAssertEqual(card.waitingOn.map(\.value), ["3 · oldest 2 h 10 min", "1 · 12 min", "2"])
        XCTAssertEqual(card.waitingOn.map(\.link), [.plans, .waiting, nil])
        XCTAssertEqual(card.youSent?.text, "\u{2068}Use Postgres\u{2069}")
        XCTAssertEqual(card.youSent?.time, "12:03")
        XCTAssertEqual(card.delivery, PmCardDelivery(text: "Read by the PM · 12:05", symbol: "checkmark.circle.fill", failed: false))
        XCTAssertNil(card.fromPM)
        XCTAssertEqual(card.fromPMEmptyText, "No message to you yet.")
    }

    func testNotDeliveredIsAFailure() throws {
        let pm = PmCard(
            agentId: "pm-1",
            fromUser: PmCard.FromUser(
                messageId: "m", createdAt: "2026-10-30T12:03:00Z", state: "expired", delivery: "not_delivered",
                body: nil))
        let card = try make(project(pm: pm))
        XCTAssertEqual(card.delivery?.text, "Not delivered (expired)")
        XCTAssertEqual(card.delivery?.failed, true)
        XCTAssertNotNil(card.youSent)
        XCTAssertNil(card.youSent?.text)
    }

    func testNewestMessageFromThePM() throws {
        let older = StoredMessage(
            key: "1", messageId: "1", sequence: 1, senderAgentId: "pm-1", messageClass: "info", state: "info",
            body: text("Old"), createdAt: "2026-10-30T11:00:00Z", requestId: "message:1")
        let newer = StoredMessage(
            key: "2", messageId: "2", sequence: 2, senderAgentId: "pm-1", messageClass: "info", state: "info",
            body: text("Merged plan-4"), createdAt: "2026-10-30T12:10:00Z", requestId: "message:2")
        let other = StoredMessage(
            key: "3", messageId: "3", sequence: 3, senderAgentId: "dev-1", messageClass: "info", state: "info",
            body: text("Not the PM"), createdAt: "2026-10-30T12:20:00Z", requestId: "message:3")
        let card = try make(project(pm: PmCard(agentId: "pm-1"), messages: [older, newer, other]))
        XCTAssertEqual(card.fromPM?.text, "\u{2068}Merged plan-4\u{2069}")
        XCTAssertEqual(card.fromPM?.time, "12:10")
        XCTAssertEqual(card.waitingEmptyText, "Nobody waits on anything.")
        XCTAssertEqual(card.youSentEmptyText, "Nothing yet. Write to pm-1 in Messages.")
    }

    func testStuckLines() throws {
        func line(_ reason: String) throws -> PmCardStuck? {
            let pm = PmCard(
                agentId: "pm-1",
                stuck: PmCard.Stuck(episodeId: "e", reason: reason, since: "2026-10-30T12:04:00Z", alert: true))
            return try make(project(pm: pm)).stuck
        }
        XCTAssertEqual(try line("unread_mail")?.title, "pm-1 looks stuck")
        XCTAssertEqual(
            try line("unread_mail")?.line, "It is idle while your message has waited unread for 17 min, since 12:04.")
        XCTAssertEqual(
            try line("no_progress")?.line, "It shows as working but has made no progress for 17 min, since 12:04.")
        XCTAssertEqual(
            try line("prompt")?.line,
            "It has been stopped at a prompt for 17 min, since 12:04. A prompt it relayed waits in Waiting for you.")
        XCTAssertEqual(try line("something_new")?.line, "pm-1 looks stuck since 12:04.")
        XCTAssertEqual(try line("prompt")?.pauseReason, "pm-1 looks stuck")
    }

    func testOfflineShowsLastKnown() throws {
        let known = try date("2026-10-30T12:15:00Z")
        let card = try make(
            project(pm: PmCard(agentId: "pm-1")),
            freshness: ProjectFreshness(liveness: .silent, lastKnownAt: known))
        XCTAssertEqual(card.freshness, .lastKnown("12:15"))
        XCTAssertEqual(card.activityWord, "Working at 12:15")
        XCTAssertEqual(card.waitingTitle, "Waiting on at 12:15")
    }

    func testFailedRefreshOnALiveHostIsStale() throws {
        let known = try date("2026-10-30T12:15:00Z")
        let card = try make(
            project(pm: PmCard(agentId: "pm-1")),
            freshness: ProjectFreshness(lastKnownAt: known, refreshFailedAt: try date("2026-10-30T12:20:00Z")))
        XCTAssertEqual(card.freshness, .stale("12:15"))
    }

    func testDurations() {
        XCTAssertEqual(PmCardModel.duration(30), "under 1 min")
        XCTAssertEqual(PmCardModel.duration(17 * 60), "17 min")
        XCTAssertEqual(PmCardModel.duration(2 * 3600), "2 h")
        XCTAssertEqual(PmCardModel.duration(26 * 3600), "1 d 2 h")
    }

    // MARK: You sent: the host's entry and the local sent log

    private func make(_ project: ProjectModel, sent: [SentMessage]) throws -> PmCardModel {
        let now = try date("2026-10-30T12:21:00Z")
        let fresh = ProjectFreshness(currentAsOf: now, lastKnownAt: now)
        return try XCTUnwrap(
            PmCardModel.make(
                project: project, projectName: "payments-api", freshness: fresh, now: now, calendar: calendar,
                sent: sent))
    }

    private func local(
        _ body: String, at time: String, state: SentMessage.State = .sending, delivery: SentMessage.Delivery? = nil
    ) throws -> SentMessage {
        SentMessage(
            hostId: UUID(), projectId: "p1", text: body, sentAt: try date(time), state: state, delivery: delivery)
    }

    private func hostSent(at time: String = "2026-10-30T12:03:00Z", id: String = "m") -> PmCard {
        PmCard(
            agentId: "pm-1",
            fromUser: PmCard.FromUser(
                messageId: id, createdAt: time, state: "sent", stateSince: time, delivery: "queued",
                body: text("From host")))
    }

    func testYouSentFromTheHostOnly() throws {
        let card = try make(project(pm: hostSent()), sent: [])
        XCTAssertEqual(card.youSent?.text, "\u{2068}From host\u{2069}")
    }

    func testYouSentFromTheLocalLogOnly() throws {
        let card = try make(
            project(pm: PmCard(agentId: "pm-1")), sent: [try local("Hello\nthere", at: "2026-10-30T12:10:00Z")])
        XCTAssertEqual(card.youSent?.text, "Hello there", "the person's own text: cleaned, not marked")
        XCTAssertEqual(card.youSent?.time, "12:10")
        XCTAssertEqual(card.delivery?.text, "Queued: the PM gets it when it is free")
    }

    func testALocalMessageNewerThanTheHostEntryWins() throws {
        let card = try make(project(pm: hostSent()), sent: [try local("Newer", at: "2026-10-30T12:10:00Z")])
        XCTAssertEqual(card.youSent?.text, "Newer")
    }

    func testAHostEntryNewerThanTheLocalMessageWins() throws {
        let card = try make(project(pm: hostSent()), sent: [try local("Older", at: "2026-10-30T11:00:00Z")])
        XCTAssertEqual(card.youSent?.text, "\u{2068}From host\u{2069}")
    }

    func testTheSameMessageIdTakesTheHostEntry() throws {
        let sameId = try local("Local copy", at: "2026-10-30T12:04:00Z", state: .sent(messageId: "m"))
        let card = try make(project(pm: hostSent()), sent: [sameId])
        XCTAssertEqual(card.youSent?.text, "\u{2068}From host\u{2069}")
        XCTAssertEqual(card.delivery?.text, "Queued: the PM gets it when it is free")
    }

    func testEachLocalDeliveryWord() throws {
        let at = try date("2026-10-30T12:05:00Z")
        let cases: [(SentMessage.State, SentMessage.Delivery?, String, Bool)] = [
            (.sending, nil, "Queued: the PM gets it when it is free", false),
            (.sent(messageId: "x"), .queued(since: at), "Queued: the PM gets it when it is free", false),
            (.sent(messageId: "x"), .delivered(at: at), "Delivered to the PM · 12:05", false),
            (.sent(messageId: "x"), .read(at: at), "Read by the PM · 12:05", false),
            (.sent(messageId: "x"), .dropped(state: "expired", at: at), "Not delivered (expired)", true),
            (.sent(messageId: "x"), .dropped(state: "odd", at: at), "Not delivered", true),
            (.notSent(reason: "no answer", pendingId: nil), nil, "Not delivered", true),
        ]
        for (state, delivery, word, failed) in cases {
            let card = try make(
                project(pm: PmCard(agentId: "pm-1")),
                sent: [try local("Hi", at: "2026-10-30T12:00:00Z", state: state, delivery: delivery)])
            XCTAssertEqual(card.delivery?.text, word)
            XCTAssertEqual(card.delivery?.failed, failed, word)
        }
    }

    func testALongLocalMessageIsCut() throws {
        let long = String(repeating: "a", count: 1000)
        let card = try make(project(pm: PmCard(agentId: "pm-1")), sent: [try local(long, at: "2026-10-30T12:00:00Z")])
        XCTAssertLessThanOrEqual(card.youSent?.text?.utf8.count ?? 0, PmCardModel.previewCap + 4)
    }
}
