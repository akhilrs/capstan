import Foundation

public enum AgentHealth: String, Codable, Sendable, CaseIterable {
    case working, needsYou, stalled, lost, blocked, paused, idle

    public var word: String {
        switch self {
        case .working: return "Working"
        case .needsYou: return "Needs you"
        case .stalled: return "Stalled"
        case .lost: return "Lost"
        case .blocked: return "Blocked"
        case .paused: return "Paused"
        case .idle: return "Idle"
        }
    }

    /// An SF Symbols name.
    public var symbolName: String {
        switch self {
        case .working: return "gearshape.fill"
        case .needsYou: return "hand.raised.fill"
        case .stalled: return "hourglass"
        case .lost: return "questionmark.circle.fill"
        case .blocked: return "xmark.octagon.fill"
        case .paused: return "pause.circle.fill"
        case .idle: return "moon.zzz.fill"
        }
    }

    /// The first case that holds, in this order: paused, lost, blocked, stalled, needs you (a prompt waits), needs you
    /// (the host sees the agent blocked), then working or idle from the host's activity. Without a usable activity
    /// the task rule decides: working when the agent is active and has a task, else idle.
    public static func of(_ agent: StoredAgent, in project: ProjectModel) -> AgentHealth {
        decide(agent, in: project).health
    }

    /// What the health rests on, for the line under the inspector's explanation.
    public static func basis(of agent: StoredAgent, in project: ProjectModel) -> HealthBasis {
        decide(agent, in: project).basis
    }

    private static func decide(_ agent: StoredAgent, in project: ProjectModel) -> (health: AgentHealth, basis: HealthBasis) {
        if agent.paused || agent.state == "paused" { return (.paused, .state) }
        if agent.state == "lost" { return (.lost, .state) }
        if agent.state == "blocked" { return (.blocked, .state) }
        if project.attention[agent.agentId] != nil { return (.stalled, .state) }
        if waitsForPrompt(agent, in: project) { return (.needsYou, .state) }
        switch agent.activity {
        case "blocked": return (.needsYou, .host(.blocked, since: agent.activitySince))
        case "working": return (.working, .host(.working, since: agent.activitySince))
        case "idle": return (.idle, .host(.idle, since: agent.activitySince))
        case "done": return (.idle, .host(.done, since: agent.activitySince))
        default: break
        }
        let reason: HealthBasis.EstimateReason
        switch agent.activity {
        case nil: reason = .absent
        case "unknown": reason = .unknown
        default: reason = .unrecognised
        }
        let health: AgentHealth = agent.state == "active" && hasTask(agent) ? .working : .idle
        return (health, .estimate(reason))
    }

    private static func hasTask(_ agent: StoredAgent) -> Bool {
        guard let task = agent.task else { return false }
        return !task.verified || !task.raw.isEmpty
    }

    private static func waitsForPrompt(_ agent: StoredAgent, in project: ProjectModel) -> Bool {
        project.waiting.values.contains { item in
            guard item.kind == .prompt, let prompt = project.details[item.key]?.promptBody else { return false }
            return prompt.agentId == agent.agentId
        }
    }
}

/// What an agent's health rests on. `state` means the controller's own facts decided; no line is shown for it.
public enum HealthBasis: Sendable, Equatable {
    public enum HostActivity: String, Sendable, Equatable {
        case idle, working, blocked, done
    }

    public enum EstimateReason: Sendable, Equatable {
        case unknown, unrecognised, absent
    }

    case state
    case host(HostActivity, since: String?)
    case estimate(EstimateReason)

    /// The line under the explanation; nil when the controller's facts decided. The host's raw activity text is never
    /// part of it.
    public func line(now: Date, locale: Locale = .current) -> String? {
        switch self {
        case .state:
            return nil
        case .host(let activity, let since):
            let clause = Self.sinceClause(since, now: now, locale: locale).map { ", " + $0 } ?? ""
            return "Host activity: " + activity.rawValue + clause + "."
        case .estimate(.unknown):
            return "Estimate: the host reported activity unknown."
        case .estimate(.unrecognised):
            return "Estimate: the host reported an activity this app does not know."
        case .estimate(.absent):
            return "Estimate: the host reported no activity."
        }
    }

    /// Nil when the time is absent, unparseable or in the future.
    static func sinceClause(_ since: String?, now: Date, locale: Locale) -> String? {
        guard let since, let date = parse(since), date <= now else { return nil }
        let minutes = Int(now.timeIntervalSince(date) / 60)
        if minutes < 1 { return "for less than a minute" }
        if minutes < 60 { return "for \(minutes) min" }
        if minutes < 24 * 60 {
            let rest = minutes % 60
            return rest == 0 ? "for \(minutes / 60) h" : "for \(minutes / 60) h \(rest) min"
        }
        let format = DateFormatter()
        format.locale = locale
        format.setLocalizedDateFormatFromTemplate("d MMM jj:mm")
        return "since " + format.string(from: date)
    }

    private static func parse(_ text: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: text) { return date }
        return ISO8601DateFormatter().date(from: text)
    }
}

extension ProjectModel {
    public func health(of agent: StoredAgent) -> AgentHealth {
        AgentHealth.of(agent, in: self)
    }
}

/// The share of a plan's packages in each stage; the four fractions add up to 1 (all 0 for a plan without packages).
public struct PlanProgress: Sendable, Equatable {
    public let integrated: Double
    public let reported: Double
    public let assigned: Double
    public let notStarted: Double

    public init(_ plan: StoredPlan) {
        let counts = plan.packages
        guard counts.total > 0 else {
            integrated = 0
            reported = 0
            assigned = 0
            notStarted = 0
            return
        }
        let total = Double(counts.total)
        // `assigned` includes `reported`, which includes `integrated`.
        let done = min(max(counts.integrated, 0), counts.total)
        let sent = min(max(counts.reported, done), counts.total)
        let started = min(max(counts.assigned, sent), counts.total)
        integrated = Double(done) / total
        reported = Double(sent - done) / total
        assigned = Double(started - sent) / total
        notStarted = Double(counts.total - started) / total
    }

    /// Packages integrated out of all packages, for a label such as 0/3.
    public static func label(_ plan: StoredPlan) -> String {
        "\(plan.packages.integrated)/\(plan.packages.total)"
    }
}
