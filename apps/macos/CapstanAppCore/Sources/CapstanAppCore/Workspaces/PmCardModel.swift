import CapstanKit
import Foundation

// The PM card of the project overview (plan-152, design-pm), as pure rules. Its words come from the run item's `pm`
// member, the PM's agent item and the PM's newest message; host text appears only in the two message previews,
// cleaned, cut and isolated.

/// Where a row of the card leads.
public enum PmCardLink: Sendable, Equatable {
    /// The conversation with the project's PM.
    case conversation
    /// The project's plans.
    case plans
    /// Waiting for you, filtered to the project.
    case waiting
}

/// A message preview: the newest from the PM, or the person's newest to it.
public struct PmCardMessage: Sendable, Equatable {
    /// Cleaned, cut and isolated; nil when the host could not vouch for the text.
    public let text: String?
    /// `12:03`, `yesterday 18:40`; empty when the time does not read.
    public let time: String
}

/// The delivery of the person's newest message to the PM.
public struct PmCardDelivery: Sendable, Equatable {
    public let text: String
    /// An SF Symbols name.
    public let symbol: String
    /// Not delivered: drawn red with a warning glyph.
    public let failed: Bool
}

/// One kind of work the PM waits on.
public struct PmCardWaitingRow: Sendable, Equatable, Identifiable {
    public var id: String { kind }
    public let kind: String
    public let title: String
    /// `3 · oldest 2 h 10 min`, `1 · 12 min`.
    public let value: String
    /// Nil for a kind the app does not know.
    public let link: PmCardLink?
}

/// The stuck banner inside the card.
public struct PmCardStuck: Sendable, Equatable, Identifiable {
    public var id: String { episodeId }
    public let episodeId: String
    /// `pm-1 looks stuck`.
    public let title: String
    /// One line from the reason; no host text.
    public let line: String
    /// The pause sheet's reason, filled in for the person and free to clear.
    public let pauseReason: String
}

/// How fresh the card's values are, beside its header.
public enum PmCardFreshness: Sendable, Equatable {
    case current
    /// The host or project cannot be read: grey `Last known · 12:31`.
    case lastKnown(String)
    /// The host is live but the last read failed: yellow `Stale · as of 12:38`, with Refresh.
    case stale(String)
}

public struct PmCardModel: Sendable, Equatable {
    public static let previewCap = 300

    /// The PM's id, cleaned; nil when no PM is active (the card then shows `noPMText`).
    public let agentId: String?
    public let noPMText: String?
    public let health: AgentHealth?
    public let activityWord: String
    public let activitySymbol: String
    /// `since 11:58 · 23 min`; nil when the time does not read.
    public let sinceText: String?
    public let freshness: PmCardFreshness
    public let stuck: PmCardStuck?

    public let fromPM: PmCardMessage?
    public let fromPMEmptyText: String
    public let youSent: PmCardMessage?
    public let delivery: PmCardDelivery?
    public let youSentEmptyText: String
    public let waitingOn: [PmCardWaitingRow]
    public let waitingTitle: String
    public let waitingEmptyText: String

    /// The card of the project, or nil while the project has no complete snapshot.
    public static func make(
        project: ProjectModel, projectName: String, freshness: ProjectFreshness, now: Date,
        calendar: Calendar = Calendar.current
    ) -> PmCardModel? {
        guard project.snapshotComplete else { return nil }
        let times = OverviewTimes(now: now, calendar: calendar)
        let fresh = freshnessOf(freshness, times: times)
        guard let pm = project.run?.pm else {
            return PmCardModel(
                agentId: nil,
                noPMText: "No PM is active on " + projectName
                    + ", so nobody routes work. The card fills in when a PM registers.",
                health: nil, activityWord: "", activitySymbol: "", sinceText: nil, freshness: fresh, stuck: nil,
                fromPM: nil, fromPMEmptyText: "", youSent: nil, delivery: nil, youSentEmptyText: "", waitingOn: [],
                waitingTitle: "", waitingEmptyText: "")
        }
        let name = HostText.name(pm.agentId)
        let agent = project.agents.values.first { $0.agentId == pm.agentId }
        let health = agent.map { AgentRow(agent: $0, in: project).health }
        let lastKnownTime: String? = {
            if case .lastKnown(let time) = fresh { return time }
            return nil
        }()
        // Offline, the word says when it was true: `Working at 12:31`.
        var word = health?.word ?? "Unknown"
        if health != nil, let lastKnownTime { word += " at " + lastKnownTime }
        return PmCardModel(
            agentId: name, noPMText: nil, health: health, activityWord: word,
            activitySymbol: health?.symbolName ?? "questionmark.circle",
            sinceText: agent.flatMap { sinceLine($0.activitySince, times: times) },
            freshness: fresh,
            stuck: pm.stuck.map { stuckBanner($0, name: name, times: times) },
            fromPM: newestFromPM(project, agentId: pm.agentId, times: times),
            fromPMEmptyText: "No message to you yet.",
            youSent: pm.fromUser.map { message(text: $0.body, at: $0.createdAt, times: times) },
            delivery: pm.fromUser.flatMap { deliveryOf($0, times: times) },
            youSentEmptyText: "Nothing yet. Write to " + name + " in Messages.",
            waitingOn: pm.waitingOn.map { waitingRow($0, times: times) },
            waitingTitle: lastKnownTime.map { "Waiting on at " + $0 } ?? "Waiting on",
            waitingEmptyText: "Nobody waits on anything.")
    }

    // MARK: Parts

    private static func freshnessOf(_ freshness: ProjectFreshness, times: OverviewTimes) -> PmCardFreshness {
        if !freshness.isReady || freshness.liveness != .live {
            guard let known = freshness.lastKnownAt ?? freshness.currentAsOf else { return .lastKnown("unknown") }
            return .lastKnown(times.text(known))
        }
        if freshness.refreshFailedAt != nil {
            guard let known = freshness.lastKnownAt ?? freshness.currentAsOf else { return .stale("unknown") }
            return .stale(times.text(known))
        }
        return .current
    }

    /// `since 11:58 · 23 min`.
    static func sinceLine(_ since: String?, times: OverviewTimes) -> String? {
        guard let start = OverviewTimes.date(since), start <= times.now else { return nil }
        return "since " + times.text(start) + " · " + duration(times.now.timeIntervalSince(start))
    }

    /// `under 1 min`, `23 min`, `2 h 10 min`, `3 d 4 h`.
    static func duration(_ seconds: TimeInterval) -> String {
        let minutes = Int(max(0, seconds) / 60)
        if minutes < 1 { return "under 1 min" }
        if minutes < 60 { return "\(minutes) min" }
        let hours = minutes / 60
        if hours < 24 {
            let rest = minutes % 60
            return rest == 0 ? "\(hours) h" : "\(hours) h \(rest) min"
        }
        let rest = hours % 24
        return rest == 0 ? "\(hours / 24) d" : "\(hours / 24) d \(rest) h"
    }

    private static func message(text: UntrustedText?, at time: String, times: OverviewTimes) -> PmCardMessage {
        let shown: String? = text.flatMap { (body: UntrustedText) -> String? in
            guard body.verified else { return nil }
            let one = HostText.clean(body.displayPlain, maxBytes: previewCap).trimmingCharacters(in: .whitespaces)
            return one.isEmpty ? nil : OverviewModel.isolate(one)
        }
        return PmCardMessage(text: shown, time: OverviewTimes.date(time).map(times.text) ?? "")
    }

    private static func newestFromPM(_ project: ProjectModel, agentId: String, times: OverviewTimes) -> PmCardMessage? {
        let mine = (project.messages ?? [:]).values.filter { $0.senderAgentId == agentId }
        guard let newest = mine.max(by: { $0.sequence < $1.sequence }) else { return nil }
        return message(text: newest.body, at: newest.createdAt, times: times)
    }

    private static func deliveryOf(_ sent: PmCard.FromUser, times: OverviewTimes) -> PmCardDelivery? {
        let at = OverviewTimes.date(sent.stateSince).map { " · " + times.text($0) } ?? ""
        switch sent.delivery {
        case "queued":
            return PmCardDelivery(text: "Queued: the PM gets it when it is free", symbol: "clock", failed: false)
        case "delivered":
            return PmCardDelivery(text: "Delivered to the PM" + at, symbol: "checkmark", failed: false)
        case "read":
            return PmCardDelivery(text: "Read by the PM" + at, symbol: "checkmark.circle.fill", failed: false)
        case "not_delivered":
            let state = sent.state.map(HostText.name) ?? ""
            let known = ["expired", "cancelled", "skipped", "failed"].contains(state)
            return PmCardDelivery(
                text: known ? "Not delivered (" + state + ")" : "Not delivered", symbol: "exclamationmark.triangle.fill",
                failed: true)
        default:
            return nil
        }
    }

    private static func waitingRow(_ entry: PmCard.Waiting, times: OverviewTimes) -> PmCardWaitingRow {
        let (title, link): (String, PmCardLink?) = {
            switch entry.kind {
            case "report": return ("Worker reports", .plans)
            case "review": return ("Reviews", .plans)
            case "verify": return ("Verify runs", .plans)
            case "plan_review": return ("Plan reviews", .plans)
            case "user": return ("You", .waiting)
            default: return (HostText.name(entry.kind), nil)
            }
        }()
        var value = String(entry.count)
        if let start = OverviewTimes.date(entry.since), start <= times.now {
            let age = duration(times.now.timeIntervalSince(start))
            value += entry.count > 1 ? " · oldest " + age : " · " + age
        }
        return PmCardWaitingRow(kind: entry.kind, title: title, value: value, link: link)
    }

    static func stuckBanner(_ stuck: PmCard.Stuck, name: String, times: OverviewTimes) -> PmCardStuck {
        let title = name + " looks stuck"
        let start = OverviewTimes.date(stuck.since).flatMap { $0 <= times.now ? $0 : nil }
        let span = start.map { " for " + duration(times.now.timeIntervalSince($0)) + ", since " + times.text($0) } ?? ""
        let line: String
        switch stuck.reason {
        case "unread_mail":
            line = "It is idle while your message has waited unread" + span + "."
        case "no_progress":
            line = "It shows as working but has made no progress" + span + "."
        case "prompt":
            line = "It has been stopped at a prompt" + span + ". A prompt it relayed waits in Waiting for you."
        default:
            line = name + " looks stuck" + (start.map { " since " + times.text($0) } ?? "") + "."
        }
        return PmCardStuck(episodeId: stuck.episodeId, title: title, line: line, pauseReason: title)
    }
}
