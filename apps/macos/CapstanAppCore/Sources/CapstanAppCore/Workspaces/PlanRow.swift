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
    /// Packages the host did not list in the plan's details.
    public let omittedPackages: Int64
    /// Packages whose review passed, from the plan's details; 0 from a host without details.
    public let reviewsPassed: Int64

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
        omittedPackages = max(plan.packageDetailsOmitted ?? 0, 0)
        reviewsPassed = Int64((plan.packageDetails ?? []).filter { $0.review == "passed" }.count)
    }

    /// The state in words: `signed_off` is `signed off`.
    public var stateWord: String { state.replacingOccurrences(of: "_", with: " ") }

    /// `5 packages`, or `1 package`.
    private var packageWord: String { total == 1 ? "1 package" : "\(total) packages" }

    /// The one meta line of a plan card: `approved · 5 packages · 2 integrated, 1 reported, 1 assigned`, or
    /// `signed off · 3 of 3 integrated` once every package is integrated. Not-started packages are left out of the
    /// counts (the bar shows them). The review round is not in the wire data, so it is not shown.
    public var metaLine: String {
        guard total > 0 else { return stateWord + " · no packages" }
        if isDone { return "\(stateWord) · \(integrated) of \(total) integrated" }
        if state == "draft" { return "draft · " + packageWord }
        var counts: [String] = []
        if integrated > 0 { counts.append("\(integrated) integrated") }
        if reportedOnly > 0 { counts.append("\(reportedOnly) reported") }
        if assignedOnly > 0 { counts.append("\(assignedOnly) assigned") }
        return ([stateWord, packageWord] + (counts.isEmpty ? [] : [counts.joined(separator: ", ")])).joined(separator: " · ")
    }

    /// The summary under the inspector's bar: `2 of 5 integrated · 2 reviews passed`. The reviewer's name and the
    /// verify result of the last report are not in the wire data, so they are not shown.
    public var summaryLine: String {
        guard total > 0 else { return "No packages" }
        var parts = ["\(integrated) of \(total) integrated"]
        if reviewsPassed > 0 { parts.append(reviewsPassed == 1 ? "1 review passed" : "\(reviewsPassed) reviews passed") }
        return parts.joined(separator: " · ")
    }

    /// The plan is in review: its state is `in_review` or `submitted`.
    public var isInReview: Bool { state == "in_review" || state == "submitted" }

    /// The pill of a plan card, only when it means something: packages waiting for you, the plan in review, or a
    /// signed-off plan ready to merge. `waiting` is how many integration items wait in the project.
    public func pill(waiting: Int) -> PlanPill? {
        if hasUnintegratedWork, waiting > 0 { return PlanPill(word: "\(waiting) waits for you", tone: .yellow) }
        if isInReview { return PlanPill(word: "In review", tone: .grey) }
        if state == "signed_off" { return PlanPill(word: "Ready to merge", tone: .green) }
        if state == "draft" { return PlanPill(word: "Draft", tone: .grey) }
        return nil
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

    /// The stored plan a row was made from. A row carries the cleaned plan id, so plans are matched on the cleaned id.
    public static func plan(for row: PlanRow, in project: ProjectModel) -> StoredPlan? {
        project.plans.values.first { HostText.name($0.planId) == row.planId }
    }

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
    /// The integration items that wait for one plan: those a package of the plan names by its integration id (the
    /// item key is `integration:<id>`), oldest first and then by key. Empty for a plan from a host that sends no
    /// package details, or none of whose packages has an integration waiting.
    public static func waiting(for plan: StoredPlan?, in project: ProjectModel) -> [StoredWaiting] {
        let ids = Set((plan?.packageDetails ?? []).compactMap { $0.integrationId })
        guard !ids.isEmpty else { return [] }
        let items: [StoredWaiting] = project.waiting.values.filter { (item: StoredWaiting) -> Bool in
            item.kind == .integration && (ids.contains(item.key) || ids.contains { item.key == "integration:\($0)" })
        }
        return items.sorted { (left: StoredWaiting, right: StoredWaiting) -> Bool in
            left.since != right.since ? left.since < right.since : left.key < right.key
        }
    }

    /// How many integration items wait in the project.
    public static func waitingCount(_ project: ProjectModel) -> Int {
        project.waiting.values.filter { $0.kind == .integration }.count
    }
}

/// A word and a tone on a plan card.
public struct PlanPill: Sendable, Equatable {
    public let word: String
    public let tone: Tone
}

/// The filter of the Plans toolbar. Done is a plan that is signed off, cancelled, superseded, rejected or failed, or
/// whose packages are all integrated. Active is the rest, a draft included (it reads `draft · N packages` with a Draft
/// pill): the design has Active and Done only. The host's plan states are draft, in_review, approved, superseded and signed_off.
public enum PlanFilter: CaseIterable, Sendable, Equatable {
    case active, done

    public var title: String {
        switch self {
        case .active: return "Active"
        case .done: return "Done"
        }
    }

    public static func filter(of row: PlanRow) -> PlanFilter {
        if row.isDone { return .done }
        switch row.state {
        case "signed_off", "cancelled", "superseded", "rejected", "failed": return .done
        default: return .active
        }
    }

    public func matches(_ row: PlanRow) -> Bool {
        PlanFilter.filter(of: row) == self
    }

    public static func counts(_ rows: [PlanRow]) -> (active: Int, done: Int) {
        (
            rows.filter { PlanFilter.active.matches($0) }.count,
            rows.filter { PlanFilter.done.matches($0) }.count
        )
    }
}
