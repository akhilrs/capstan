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

    /// The first case that holds, in this order: paused, lost, blocked, stalled, needs you, working, idle.
    public static func of(_ agent: StoredAgent, in project: ProjectModel) -> AgentHealth {
        if agent.paused || agent.state == "paused" { return .paused }
        if agent.state == "lost" { return .lost }
        if agent.state == "blocked" { return .blocked }
        if project.attention[agent.agentId] != nil { return .stalled }
        if waitsForPrompt(agent, in: project) { return .needsYou }
        if agent.state == "active", hasTask(agent) { return .working }
        return .idle
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
