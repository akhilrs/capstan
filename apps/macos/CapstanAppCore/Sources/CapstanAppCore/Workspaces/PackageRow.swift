import CapstanKit
import Foundation

/// One package of a plan as the plan's detail lists it. The host sends the packages in its own order (live ones first,
/// cancelled ones last) and the rows keep that order.
public struct PackageRow: Sendable, Equatable {
    public let id: String
    public let title: String
    public let stateWord: String
    public let stateSymbol: String
    /// Passed, Changes requested or Not reviewed.
    public let reviewWord: String
    public let dependsOn: [String]
    public let assignee: String?
    /// Merged or Conflicted while an integration waits, nil otherwise.
    public let integrationWord: String?
    /// The pill of the design: Integrated, Reported, Waits for you, Assigned, Not started or Cancelled.
    public let pillWord: String
    public let pillTone: Tone
    /// The leading icon: a check, a clock, a circle or a dashed circle.
    public let statusIcon: String
    /// The branch of the assignee's pane; nil when nobody holds the package or the host did not send it.
    public let branch: String?
    /// The words after the branch: the review state, then the order (`after timeouts`, `no dependencies`,
    /// `blocked by a and b`). The reviewer's name, the review round and the verify result are not in the wire data.
    public let detailLine: String

    public init(_ detail: PackageDetail, in plan: [PackageDetail] = [], agent: AgentRow? = nil) {
        id = HostText.name(detail.id)
        let shown = HostText.clean(detail.title.displayPlain, maxBytes: HostText.codeCap)
        title = detail.title.verified && !shown.trimmingCharacters(in: .whitespaces).isEmpty ? shown : id
        let shownState = PackageRow.state(detail.state)
        stateWord = shownState.0
        stateSymbol = shownState.1
        switch detail.review {
        case "passed": reviewWord = "Passed"
        case "findings": reviewWord = "Changes requested"
        default: reviewWord = "Not reviewed"
        }
        dependsOn = detail.dependsOn.map { HostText.name($0) }
        assignee = detail.assigneeAgentId.map { HostText.name($0) }
        integrationWord = detail.integrationState.map { PackageRow.integration($0) }
        let waits = agent?.health == .needsYou && detail.state == .assigned
        let pill = PackageRow.pill(detail.state, waits: waits)
        pillWord = pill.word
        pillTone = pill.tone
        statusIcon = PackageRow.icon(detail.state)
        branch = agent?.branch
        let review: String
        switch detail.review {
        case "passed": review = "review passed"
        case "findings": review = "review: changes requested"
        default: review = "not reviewed"
        }
        var parts = [String]()
        if waits { parts.append("prompt open") }
        if branch == nil, detail.state == .unassigned {
            parts.append("no branch yet")
        } else {
            parts.append(review)
        }
        parts.append(PackageRow.order(detail, in: plan))
        detailLine = parts.joined(separator: " · ")
    }

    private static func pill(_ state: PackageState, waits: Bool) -> (word: String, tone: Tone) {
        switch state {
        case .integrated, .confirmed: return ("Integrated", .green)
        case .reported: return ("Reported", .blue)
        case .assigned: return waits ? ("Waits for you", .yellow) : ("Assigned", .grey)
        case .unassigned: return ("Not started", .grey)
        case .cancelled: return ("Cancelled", .grey)
        case .other(let name):
            let word = HostText.name(name)
            return (word.isEmpty ? "Unknown" : word, .grey)
        }
    }

    private static func icon(_ state: PackageState) -> String {
        switch state {
        case .integrated, .confirmed: return "checkmark"
        case .reported: return "clock"
        case .assigned: return "circle"
        case .unassigned: return "circle.dashed"
        case .cancelled: return "xmark"
        case .other: return "questionmark.circle"
        }
    }

    /// `no dependencies`, `after a and b` when every dependency is integrated, `blocked by a and b` naming those that
    /// are not (a package that is already started is only ever `after`).
    static func order(_ detail: PackageDetail, in plan: [PackageDetail]) -> String {
        guard !detail.dependsOn.isEmpty else { return "no dependencies" }
        let names = detail.dependsOn.map { HostText.name($0) }
        let unmet = detail.dependsOn.filter { id in
            let state = plan.first { $0.id == id }?.state
            return state != .integrated && state != .confirmed
        }.map { HostText.name($0) }
        if detail.state == .unassigned, !unmet.isEmpty { return "blocked by " + list(unmet) }
        return "after " + list(names)
    }

    private static func list(_ names: [String]) -> String {
        guard names.count > 1, let last = names.last else { return names.joined() }
        return names.dropLast().joined(separator: ", ") + " and " + last
    }

    private static func state(_ state: PackageState) -> (String, String) {
        switch state {
        case .unassigned: return ("Not started", "circle")
        case .assigned: return ("Assigned", "person.circle")
        case .reported: return ("Reported", "doc.text")
        case .integrated: return ("Integrated", "checkmark.circle")
        case .confirmed: return ("Confirmed", "checkmark.seal")
        case .cancelled: return ("Cancelled", "xmark.circle")
        case .other(let name):
            let word = HostText.name(name)
            return (word.isEmpty ? "Unknown" : word, "questionmark.circle")
        }
    }

    private static func integration(_ state: String) -> String {
        switch state {
        case "merged": return "Merged"
        case "conflicted": return "Conflicted"
        default: return HostText.name(state)
        }
    }

    /// The packages of a plan in the host's order; empty from a host that does not send details.
    public static func rows(_ plan: StoredPlan, in project: ProjectModel? = nil) -> [PackageRow] {
        let details = plan.packageDetails ?? []
        let agents: [AgentRow] = project.map { AgentRow.rows($0) } ?? []
        return details.map { (detail: PackageDetail) -> PackageRow in
            let agent = detail.assigneeAgentId.flatMap { id in agents.first { $0.agentId == HostText.name(id) } }
            return PackageRow(detail, in: details, agent: agent)
        }
    }
}
