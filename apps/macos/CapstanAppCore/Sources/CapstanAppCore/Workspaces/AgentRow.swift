import CapstanKit
import Foundation

/// One agent as the Agents screen lists it. Host text is cleaned and cut; the health is the kit's.
public struct AgentRow: Sendable, Equatable {
    public static let taskCap = 256

    /// The agent as stored: its raw id and request id are for sending decisions, never for display.
    public let source: StoredAgent
    public let agentId: String
    public let role: String
    public let state: String
    /// The task on one line; nil when there is none or the host could not vouch for it.
    public let task: String?
    public let taskUnverified: Bool
    /// The branch of the agent's pane on one line; nil when there is none or the host could not vouch for it.
    public let branch: String?
    public let health: AgentHealth
    public let paused: Bool

    public var healthWord: String { health.word }
    public var healthSymbol: String { health.symbolName }

    public init(agent: StoredAgent, health: AgentHealth) {
        source = agent
        agentId = HostText.name(agent.agentId)
        role = HostText.name(agent.role)
        state = HostText.name(agent.state)
        paused = agent.paused
        // A paused agent shows Paused whatever the kit computed from the other fields.
        self.health = agent.paused ? .paused : health
        if let text = agent.branch, text.verified {
            let one = HostText.clean(text.displayPlain, maxBytes: AgentRow.taskCap)
            branch = one.trimmingCharacters(in: .whitespaces).isEmpty ? nil : one
        } else {
            branch = nil
        }
        if let text = agent.task {
            taskUnverified = !text.verified
            let one = HostText.clean(text.displayPlain, maxBytes: AgentRow.taskCap)
            task = text.verified && !one.trimmingCharacters(in: .whitespaces).isEmpty ? one : nil
        } else {
            taskUnverified = false
            task = nil
        }
    }

    public init(agent: StoredAgent, in project: ProjectModel) {
        self.init(agent: agent, health: project.health(of: agent))
    }

    /// The rows of a project, by role and then agent id.
    public static func rows(_ project: ProjectModel) -> [AgentRow] {
        let agents: [StoredAgent] = Array(project.agents.values)
        let pairs: [(agent: StoredAgent, row: AgentRow)] = agents.map { (agent: StoredAgent) -> (agent: StoredAgent, row: AgentRow) in
            (agent: agent, row: AgentRow(agent: agent, in: project))
        }
        let ordered: [(agent: StoredAgent, row: AgentRow)] = pairs.sorted(by: precedes)
        return ordered.map { (pair: (agent: StoredAgent, row: AgentRow)) -> AgentRow in pair.row }
    }

    private static func precedes(
        _ left: (agent: StoredAgent, row: AgentRow), _ right: (agent: StoredAgent, row: AgentRow)
    ) -> Bool {
        if left.row.role == right.row.role { return left.agent.agentId < right.agent.agentId }
        return left.row.role.localizedStandardCompare(right.row.role) == .orderedAscending
    }
}

/// The filter of the Agents toolbar: All, Need attention and Paused.
public enum AgentFilter: CaseIterable, Sendable, Equatable {
    case all, needAttention, paused

    public var title: String {
        switch self {
        case .all: return "All"
        case .needAttention: return "Need attention"
        case .paused: return "Paused"
        }
    }

    public func matches(_ row: AgentRow) -> Bool {
        switch self {
        case .all: return true
        case .paused: return row.health == .paused
        case .needAttention:
            switch row.health {
            case .needsYou, .stalled, .lost, .blocked: return true
            default: return false
            }
        }
    }

    public func count(_ rows: [AgentRow]) -> Int { rows.filter(matches).count }
}
