import CapstanKit
import Foundation

/// One project's conversation in the Messages list.
public struct Conversation: Identifiable, Sendable, Equatable {
    public let hostId: UUID
    public let projectId: String
    public let projectName: String
    public let hostName: String
    /// The active PM; nil when the project has none (nothing can be sent).
    public let pm: String?
    /// PM messages waiting for an answer.
    public let waiting: Int
    /// The newest line: "pm-1: …" or "You: … · Sent".
    public let last: String
    public let lastAt: Date?
    public var id: String { hostId.uuidString + "/" + projectId }

    public init(
        hostId: UUID, projectId: String, projectName: String, hostName: String, pm: String?, waiting: Int,
        last: String, lastAt: Date?
    ) {
        self.hostId = hostId
        self.projectId = projectId
        self.projectName = projectName
        self.hostName = hostName
        self.pm = pm
        self.waiting = waiting
        self.last = last
        self.lastAt = lastAt
    }
}

public enum Conversations {
    /// The conversations: every ready project with an active PM, a message from an agent or a sent message, newest
    /// first. `waiting` counts the questions not answered yet. Pure: the caller passes the hosts, their models and
    /// the sent messages.
    public static func make(hosts: [HostConfig], hostModels: [UUID: HostModel], sent log: SentMessageLog) -> [Conversation] {
        var rows: [Conversation] = []
        for host in hosts {
            for project in (hostModels[host.id]?.projects.values).map(Array.init) ?? [] where project.availability == .ready {
                let pm = PmBinding.activePm(project)
                let incoming = project.waiting.values.filter { $0.kind == .message }
                let received = Array((project.messages ?? [:]).values)
                let sent = log.thread(hostId: host.id, projectId: project.projectId)
                guard pm != nil || !incoming.isEmpty || !received.isEmpty || !sent.isEmpty else { continue }
                // The newest message from an agent: from the `messages` section, else a waiting message item.
                let newestReceived = received.max { $0.sequence < $1.sequence }
                let newestIn = incoming.max { $0.since < $1.since }
                let inText: UntrustedText?
                let inFrom: String
                let inAt: Date?
                if let newestReceived {
                    inText = newestReceived.body
                    inFrom = newestReceived.senderAgentId
                    inAt = SentMessageLog.parseTime(newestReceived.createdAt)
                } else {
                    inText = newestIn?.summary
                    inFrom = pm ?? "PM"
                    inAt = newestIn.flatMap { SentMessageLog.parseTime($0.since) }
                }
                let newestOut = sent.last
                let last: String
                let lastAt: Date?
                if let newestOut, inAt.map({ newestOut.sentAt >= $0 }) ?? true {
                    last = "You: " + newestOut.text + " · " + MessageStateText.short(newestOut.state)
                    lastAt = newestOut.sentAt
                } else if let inText {
                    let text = inText.verified
                        ? TextBudget.displayPrefix(inText.displayPlain, maxBytes: 200).text
                        : "A message that failed its integrity check"
                    last = inFrom + ": " + text
                    lastAt = inAt
                } else {
                    last = "No messages yet"
                    lastAt = nil
                }
                let open = received.isEmpty ? incoming.count : received.filter(\.isOpenQuestion).count
                rows.append(Conversation(
                    hostId: host.id, projectId: project.projectId,
                    projectName: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text, hostName: host.displayName,
                    pm: pm, waiting: open, last: last, lastAt: lastAt))
            }
        }
        return rows.sorted { ($0.lastAt ?? .distantPast) > ($1.lastAt ?? .distantPast) }
    }
}

/// The words of a sent message's state.
public enum MessageStateText {
    public static func short(_ state: SentMessage.State) -> String {
        switch state {
        case .sending: return "Sending"
        case .sent: return "Sent"
        case .notSent: return "Not sent"
        }
    }

    public static func symbol(_ delivery: SentMessage.Delivery) -> String {
        switch delivery {
        case .queued: return "clock"
        case .delivered: return "checkmark"
        case .read: return "checkmark.circle.fill"
        case .dropped: return "xmark.circle"
        }
    }

    /// The host's delivery of a sent message, in a few words; nil before the host said.
    public static func delivery(_ message: SentMessage, now: Date) -> String? {
        switch message.delivery {
        case .queued(let since)?:
            let minutes = Int(now.timeIntervalSince(since) / 60)
            return message.stuck(now: now)
                ? "Waiting for the PM for " + String(minutes) + " min: it is busy, stuck at a question, or paused"
                : "Queued: the PM gets it when it is free"
        case .delivered?: return "Delivered to the PM"
        case .read?: return "Read by the PM"
        case .dropped(let state, _)?: return "Not delivered (" + state + ")"
        case nil: return nil
        }
    }
}
