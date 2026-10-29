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

    public init(_ detail: PackageDetail) {
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
    public static func rows(_ plan: StoredPlan) -> [PackageRow] {
        (plan.packageDetails ?? []).map { (detail: PackageDetail) -> PackageRow in PackageRow(detail) }
    }
}
