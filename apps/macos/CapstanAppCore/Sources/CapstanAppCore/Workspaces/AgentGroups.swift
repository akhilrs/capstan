import CapstanKit
import Foundation

/// The agents of one project, as the Agents tab groups them.
public struct AgentGroup: Identifiable, Sendable, Equatable {
    public let hostId: UUID
    public let projectId: String
    public let projectName: String
    public let hostName: String
    public let rows: [AgentRow]
    public var id: String { hostId.uuidString + "/" + projectId }

    public init(hostId: UUID, projectId: String, projectName: String, hostName: String, rows: [AgentRow]) {
        self.hostId = hostId
        self.projectId = projectId
        self.projectName = projectName
        self.hostName = hostName
        self.rows = rows
    }

    /// The agents of every ready project, by host name and then project name. Pure: the caller passes the values.
    public static func make(hosts: [HostConfig], hostModels: [UUID: HostModel]) -> [AgentGroup] {
        var groups: [AgentGroup] = []
        for host in hosts {
            guard let projects = hostModels[host.id]?.projects.values else { continue }
            for project in projects where project.availability == .ready && !project.agents.isEmpty {
                groups.append(AgentGroup(
                    hostId: host.id, projectId: project.projectId,
                    projectName: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text,
                    hostName: host.displayName, rows: AgentRow.rows(project)))
            }
        }
        return groups.sorted { left, right in
            if left.hostName != right.hostName {
                return left.hostName.localizedStandardCompare(right.hostName) == .orderedAscending
            }
            return left.projectName.localizedStandardCompare(right.projectName) == .orderedAscending
        }
    }

    /// The group as a line of the jump sheet.
    public var jumpEntry: JumpEntry {
        let counts = HealthCounts(rows)
        return JumpEntry(
            id: id, name: projectName, hostName: hostName,
            count: rows.count == 1 ? "1 agent" : String(rows.count) + " agents",
            state: counts.headline, tone: counts.tone)
    }
}

/// An agent that needs attention, with its project.
public struct AttentionItem: Identifiable, Sendable, Equatable {
    public let group: AgentGroup
    public let row: AgentRow
    public var id: String { group.id + "/" + row.source.key }

    public init(group: AgentGroup, row: AgentRow) {
        self.group = group
        self.row = row
    }
}

/// What the Agents tab shows: every agent, those that need attention (needs you, stalled, lost, blocked), those
/// working, or those paused.
public enum AgentShow: CaseIterable, Hashable, Sendable {
    case all, attention, working, paused

    public var title: String {
        switch self {
        case .all: return "All"
        case .attention: return "Attention"
        case .working: return "Working"
        case .paused: return "Paused"
        }
    }

    public func matches(_ row: AgentRow) -> Bool {
        switch self {
        case .all: return true
        case .attention: return AgentFilter.needAttention.matches(row)
        case .working: return row.health == .working
        case .paused: return row.health == .paused
        }
    }

    /// The order inside a project: needs you, then stalled, lost or blocked, then working, paused and idle.
    public static func rank(_ health: AgentHealth) -> Int {
        switch health {
        case .needsYou: return 0
        case .stalled, .lost, .blocked: return 1
        case .working: return 2
        case .paused: return 3
        case .idle: return 4
        }
    }

    /// A project's agents that pass the filter and the search, those that need attention first. A search that
    /// matches the project or host name keeps every agent of it that passes the filter.
    public static func matching(_ group: AgentGroup, show: AgentShow, needle: String) -> [AgentRow] {
        let filtered = group.rows.filter(show.matches)
        let rows: [AgentRow]
        if needle.isEmpty
            || group.projectName.localizedCaseInsensitiveContains(needle)
            || group.hostName.localizedCaseInsensitiveContains(needle) {
            rows = filtered
        } else {
            rows = filtered.filter {
                $0.agentId.localizedCaseInsensitiveContains(needle) || $0.role.localizedCaseInsensitiveContains(needle)
            }
        }
        return rows.sorted { rank($0.health) < rank($1.health) }
    }

    /// Every agent that needs attention, in every project: needs you first, then the rest.
    public static func attention(_ groups: [AgentGroup]) -> [AttentionItem] {
        groups.flatMap { group in group.rows.filter(AgentShow.attention.matches).map { AttentionItem(group: group, row: $0) } }
            .sorted { rank($0.row.health) < rank($1.row.health) }
    }

    /// Project name, then host name, in the order people read them.
    public static func byName(_ left: AgentGroup, _ right: AgentGroup) -> Bool {
        if left.projectName != right.projectName {
            return left.projectName.localizedStandardCompare(right.projectName) == .orderedAscending
        }
        return left.hostName.localizedStandardCompare(right.hostName) == .orderedAscending
    }

    /// How many agents need attention, in every project: the badge of Agents in the sidebar.
    public static func attentionCount(_ groups: [AgentGroup]) -> Int {
        groups.reduce(0) { $0 + $1.rows.filter(AgentShow.attention.matches).count }
    }

    /// The agents each filter would show for the search, in every project: the numbers on the filter. A search that
    /// names a project or host keeps all its agents, as `matching` does.
    public static func counts(_ groups: [AgentGroup], needle: String) -> [AgentShow: Int] {
        var counts: [AgentShow: Int] = [:]
        for show in AgentShow.allCases {
            counts[show] = groups.reduce(0) { $0 + matching($1, show: show, needle: needle).count }
        }
        return counts
    }

    /// Why the list is empty, or nil when something shows: no agents at all, nothing under the filter, or no match.
    public static func emptiness(all: Int, shown: Int, show: AgentShow, needle: String) -> AgentEmptiness? {
        if all == 0 { return .noAgents }
        if shown > 0 { return nil }
        return needle.isEmpty ? .nothingForFilter(show) : .noMatch(needle)
    }

    public static func summary(_ agents: Int, projects: Int) -> String {
        let a = agents == 1 ? "1 agent" : String(agents) + " agents"
        let p = projects == 1 ? "1 project" : String(projects) + " projects"
        return a + " · " + p
    }
}

/// A project's agents counted by health, in words and as a bar.
public struct HealthCounts: Sendable, Equatable {
    private var counts: [AgentHealth: Int] = [:]
    public let total: Int

    public init(_ rows: [AgentRow]) {
        total = rows.count
        for row in rows { counts[row.health, default: 0] += 1 }
    }

    public func count(_ health: AgentHealth) -> Int { counts[health] ?? 0 }

    /// "1 needs you · 1 lost": what needs attention, empty when nothing does.
    public var alert: String {
        [AgentHealth.needsYou, .stalled, .lost, .blocked].compactMap { health in
            count(health) > 0 ? String(count(health)) + " " + health.word.lowercased() : nil
        }.joined(separator: " · ")
    }

    /// "2 working · 4 idle", or "all idle".
    public var rest: String {
        var parts: [String] = []
        if count(.working) > 0 { parts.append(String(count(.working)) + " working") }
        if count(.paused) > 0 { parts.append(String(count(.paused)) + " paused") }
        if count(.idle) > 0 && count(.idle) == total {
            parts.append("all idle")
        } else if count(.idle) > 0 {
            parts.append(String(count(.idle)) + " idle")
        }
        return parts.joined(separator: " · ")
    }

    /// The tone of the most urgent health.
    public var tone: Tone {
        if count(.needsYou) > 0 { return .accent }
        if count(.stalled) + count(.lost) + count(.blocked) > 0 { return .red }
        if count(.working) > 0 { return .green }
        return .grey
    }

    /// One short word group for the jump sheet: the most urgent count.
    public var headline: String {
        if !alert.isEmpty { return alert.components(separatedBy: " · ").first ?? alert }
        if count(.working) > 0 { return String(count(.working)) + " working" }
        if count(.paused) == total { return "All paused" }
        return "All idle"
    }

    /// The bar's segments, in order: needs you, stalled or lost or blocked, working, paused, idle.
    public var barCounts: [Int] {
        [
            count(.needsYou),
            count(.stalled) + count(.lost) + count(.blocked),
            count(.working),
            count(.paused),
            count(.idle),
        ]
    }
}

/// The three causes of an empty Agents list.
public enum AgentEmptiness: Sendable, Equatable {
    case noAgents
    case nothingForFilter(AgentShow)
    case noMatch(String)

    public var title: String {
        switch self {
        case .noAgents: return "No agents"
        case .nothingForFilter: return "Nothing to show"
        case .noMatch(let needle): return "No results for \"" + needle + "\""
        }
    }

    public var detail: String {
        switch self {
        case .noAgents: return "The agents of your ready projects appear here."
        case .nothingForFilter(let show):
            switch show {
            case .attention: return "No agent needs attention."
            case .working: return "No agent is working."
            case .paused: return "No agent is paused."
            case .all: return "No agent matches this filter."
            }
        case .noMatch: return "Search matches agent ids, roles, project names and host names."
        }
    }
}
