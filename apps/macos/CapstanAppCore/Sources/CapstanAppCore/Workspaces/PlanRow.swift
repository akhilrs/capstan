import CapstanKit
import Foundation

/// One plan as the Plans screen lists it: its title, and its packages counted by stage.
/// The kernel counts cumulatively (assigned includes reported, reported includes integrated).
public struct PlanRow: Sendable, Equatable {
    public let planId: String
    public let state: String
    public let sequence: Int64
    public let title: String
    public let titleUnverified: Bool
    public let total: Int64
    public let integrated: Int64
    /// Reported and not yet integrated.
    public let reportedOnly: Int64
    /// Assigned and not yet reported.
    public let assignedOnly: Int64
    public let notStarted: Int64
    public let progress: PlanProgress

    public init(_ plan: StoredPlan) {
        planId = HostText.name(plan.planId)
        state = HostText.name(plan.state)
        sequence = plan.sequence
        titleUnverified = !plan.title.verified
        let shown = HostText.clean(plan.title.displayPlain, maxBytes: HostText.codeCap)
        title = plan.title.verified && !shown.trimmingCharacters(in: .whitespaces).isEmpty ? shown : planId
        let counts = plan.packages
        total = max(counts.total, 0)
        // Same clamping as PlanProgress, so the words and the bar agree.
        let done = min(max(counts.integrated, 0), total)
        let sent = min(max(counts.reported, done), total)
        let started = min(max(counts.assigned, sent), total)
        integrated = done
        reportedOnly = sent - done
        assignedOnly = started - sent
        notStarted = total - started
        progress = PlanProgress(plan)
    }

    /// N of M integrated, then N reported, N assigned and N not started for each bucket above 0.
    public var words: String {
        guard total > 0 else { return "No packages" }
        var parts = ["\(integrated) of \(total) integrated"]
        if reportedOnly > 0 { parts.append("\(reportedOnly) reported") }
        if assignedOnly > 0 { parts.append("\(assignedOnly) assigned") }
        if notStarted > 0 { parts.append("\(notStarted) not started") }
        return parts.joined(separator: ", ")
    }

    /// Every package is integrated.
    public var isDone: Bool { total > 0 && integrated == total }

    /// Packages that are reported and not integrated yet: where a waiting integration item may belong.
    public var hasUnintegratedWork: Bool { reportedOnly > 0 }

    /// The rows of a project in plan order.
    public static func rows(_ project: ProjectModel) -> [PlanRow] {
        let plans: [StoredPlan] = Array(project.plans.values)
        let ordered: [StoredPlan] = plans.sorted { (left: StoredPlan, right: StoredPlan) -> Bool in
            if left.sequence != right.sequence { return left.sequence < right.sequence }
            return left.planId < right.planId
        }
        return ordered.map { (plan: StoredPlan) -> PlanRow in PlanRow(plan) }
    }
}

public enum IntegrationLink {
    /// How many integration items wait in the project.
    public static func waitingCount(_ project: ProjectModel) -> Int {
        project.waiting.values.filter { $0.kind == .integration }.count
    }
}

/// The filter of the Plans toolbar. Active is a plan that is approved and not finished; every plan that is not approved
/// yet (a draft, or in review) is under Drafts; Done is a signed-off plan or one whose packages are all integrated.
public enum PlanFilter: CaseIterable, Sendable, Equatable {
    case active, drafts, done

    public var title: String {
        switch self {
        case .active: return "Active"
        case .drafts: return "Drafts"
        case .done: return "Done"
        }
    }

    public static func filter(of row: PlanRow) -> PlanFilter {
        if row.isDone || row.state == "signed_off" { return .done }
        return row.state == "approved" ? .active : .drafts
    }

    public func matches(_ row: PlanRow) -> Bool {
        PlanFilter.filter(of: row) == self
    }

    public static func counts(_ rows: [PlanRow]) -> (active: Int, drafts: Int, done: Int) {
        (
            rows.filter { PlanFilter.active.matches($0) }.count,
            rows.filter { PlanFilter.drafts.matches($0) }.count,
            rows.filter { PlanFilter.done.matches($0) }.count
        )
    }
}
