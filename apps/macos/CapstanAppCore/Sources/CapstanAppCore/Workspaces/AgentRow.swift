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
        project.agents.values
            .map { (agent: $0, row: AgentRow(agent: $0, in: project)) }
            .sorted {
                $0.row.role == $1.row.role
                    ? $0.agent.agentId < $1.agent.agentId
                    : $0.row.role.localizedStandardCompare($1.row.role) == .orderedAscending
            }
            .map { $0.row }
    }
}
