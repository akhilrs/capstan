import CapstanKit
import Foundation

/// One plan as the Plans screen lists it: its title, and its packages counted by stage.
/// The kernel counts cumulatively (assigned includes reported, reported includes integrated).
public struct PlanRow: Sendable, Equatable {
    public let planId: String
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

    /// Packages that are reported and not integrated yet: where a waiting integration item may belong.
    public var hasUnintegratedWork: Bool { reportedOnly > 0 }

    /// The rows of a project in plan order.
    public static func rows(_ project: ProjectModel) -> [PlanRow] {
        project.plans.values.sorted { ($0.sequence, $0.planId) < ($1.sequence, $1.planId) }.map { PlanRow($0) }
    }
}

public enum IntegrationLink {
    /// How many integration items wait in the project.
    public static func waitingCount(_ project: ProjectModel) -> Int {
        project.waiting.values.filter { $0.kind == .integration }.count
    }
}
