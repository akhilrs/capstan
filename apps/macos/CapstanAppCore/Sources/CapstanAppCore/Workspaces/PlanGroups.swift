import CapstanKit
import Foundation

/// The plans of one project, as the Plans tab groups them.
public struct PlanGroup: Identifiable {
    public let hostId: UUID
    public let projectId: String
    public let projectName: String
    public let hostName: String
    public let rows: [PlanRow]
    public let project: ProjectModel
    public var id: String { hostId.uuidString + "/" + projectId }

    public init(
        hostId: UUID, projectId: String, projectName: String, hostName: String, rows: [PlanRow], project: ProjectModel
    ) {
        self.hostId = hostId
        self.projectId = projectId
        self.projectName = projectName
        self.hostName = hostName
        self.rows = rows
        self.project = project
    }

    /// How many integration items of the plan wait for you.
    public func waiting(_ row: PlanRow) -> Int {
        IntegrationLink.waiting(for: PlanRow.plan(for: row, in: project), in: project).count
    }

    /// The plans of every ready project, by host name and then project name. Pure: the caller passes the values.
    public static func make(hosts: [HostConfig], hostModels: [UUID: HostModel]) -> [PlanGroup] {
        var groups: [PlanGroup] = []
        for host in hosts {
            guard let projects = hostModels[host.id]?.projects.values else { continue }
            for project in projects where project.availability == .ready && !project.plans.isEmpty {
                groups.append(PlanGroup(
                    hostId: host.id, projectId: project.projectId,
                    projectName: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text,
                    hostName: host.displayName, rows: PlanRow.rows(project), project: project))
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
        let waits = rows.filter { $0.hasUnintegratedWork && waiting($0) > 0 }.count
        let active = rows.filter { PlanShow.of($0) == .active }.count
        let state: String
        let tone: Tone
        if waits > 0 {
            state = waits == 1 ? "1 waits for you" : String(waits) + " wait for you"
            tone = .accent
        } else if active > 0 {
            state = String(active) + " active"
            tone = .green
        } else {
            state = "None active"
            tone = .grey
        }
        return JumpEntry(
            id: id, name: projectName, hostName: hostName,
            count: rows.count == 1 ? "1 plan" : String(rows.count) + " plans",
            state: state, tone: tone)
    }
}

/// A plan with its project and how many integration items of it wait for you.
public struct PlanItem: Identifiable {
    public let group: PlanGroup
    public let row: PlanRow
    public let waiting: Int
    public var id: String { group.id + "/" + row.planId }

    public init(group: PlanGroup, row: PlanRow, waiting: Int) {
        self.group = group
        self.row = row
        self.waiting = waiting
    }
}

/// A group of plans in the list.
public struct PlanSection: Identifiable {
    public let id: String
    public let title: String
    public let subtitle: String?
    public let tone: Tone?
    public let items: [PlanItem]
    public let openByDefault: Bool

    public init(id: String, title: String, subtitle: String?, tone: Tone?, items: [PlanItem], openByDefault: Bool) {
        self.id = id
        self.title = title
        self.subtitle = subtitle
        self.tone = tone
        self.items = items
        self.openByDefault = openByDefault
    }

    public var total: Int64 { items.reduce(0) { $0 + $1.row.total } }
    public var integrated: Int64 { items.reduce(0) { $0 + $1.row.integrated } }
}

/// Active, In review or Done. Done is PlanFilter's done (signed off, cancelled, superseded, rejected, failed, or every
/// package integrated); In review is a plan in review or submitted; Active is the rest, drafts included.
public enum PlanShow: CaseIterable, Hashable, Sendable {
    case active, review, done

    public var title: String {
        switch self {
        case .active: return "Active"
        case .review: return "In review"
        case .done: return "Done"
        }
    }

    public static func of(_ row: PlanRow) -> PlanShow {
        if PlanFilter.filter(of: row) == .done { return .done }
        return row.isInReview ? .review : .active
    }

    /// The groups of the list. For all projects: one per project. For one project on Active: waits for you, in
    /// progress, drafts. Otherwise one group.
    public static func sections(_ items: [PlanItem], groups: [PlanGroup], scope: String?, show: PlanShow) -> [PlanSection] {
        if scope == nil {
            return groups.compactMap { group in
                let mine = items.filter { $0.group.id == group.id }
                guard !mine.isEmpty else { return nil }
                return PlanSection(id: group.id, title: group.projectName, subtitle: group.hostName,
                                   tone: nil, items: mine, openByDefault: false)
            }
        }
        switch show {
        case .active:
            let waits = items.filter { $0.waiting > 0 && $0.row.hasUnintegratedWork }
            let drafts = items.filter { $0.row.state == "draft" }
            let going = items.filter { item in
                item.row.state != "draft" && !waits.contains { $0.id == item.id }
            }
            return [
                PlanSection(id: "waits", title: "Waits for you", subtitle: nil, tone: .accent, items: waits, openByDefault: true),
                PlanSection(id: "going", title: "In progress", subtitle: nil, tone: nil, items: going, openByDefault: true),
                PlanSection(id: "drafts", title: "Drafts", subtitle: nil, tone: nil, items: drafts, openByDefault: false),
            ].filter { !$0.items.isEmpty }
        case .review:
            return items.isEmpty ? [] : [
                PlanSection(id: "review", title: "Waiting for review", subtitle: nil, tone: nil, items: items, openByDefault: true)]
        case .done:
            return items.isEmpty ? [] : [
                PlanSection(id: "done", title: "Finished", subtitle: nil, tone: nil, items: items, openByDefault: true)]
        }
    }

    public static func groupTitle(scope: String?, show: PlanShow) -> String {
        if scope == nil { return show.title + " · by project" }
        return show == .active ? "Active · by state" : show.title
    }

    /// Pinned projects first, in pin order; then the others A to Z.
    public static func ordered(_ groups: [PlanGroup], pinned: [String]) -> [PlanGroup] {
        let first = pinned.compactMap { id in groups.first { $0.id == id } }
        let rest = groups.filter { !pinned.contains($0.id) }.sorted { left, right in
            if left.projectName != right.projectName {
                return left.projectName.localizedStandardCompare(right.projectName) == .orderedAscending
            }
            return left.hostName.localizedStandardCompare(right.hostName) == .orderedAscending
        }
        return first + rest
    }

    /// Plans with integration items waiting for you, in the order given: the "Needs you" card.
    public static func needsYou(_ items: [PlanItem]) -> [PlanItem] {
        items.filter { $0.waiting > 0 && $0.row.hasUnintegratedWork }
    }

    /// The plans whose title or id holds the search; all of them when it is empty.
    public static func matching(_ items: [PlanItem], needle: String) -> [PlanItem] {
        guard !needle.isEmpty else { return items }
        return items.filter {
            $0.row.title.localizedCaseInsensitiveContains(needle) || $0.row.planId.localizedCaseInsensitiveContains(needle)
        }
    }

    /// The plans of each tab for the search: the numbers on the filter.
    public static func counts(_ items: [PlanItem], needle: String) -> [PlanShow: Int] {
        var counts: [PlanShow: Int] = [:]
        for item in matching(items, needle: needle) { counts[PlanShow.of(item.row), default: 0] += 1 }
        return counts
    }

    public static func summary(_ plans: Int, projects: Int, scope: String?, all: Bool) -> String {
        let p = plans == 1 ? "1 plan" : String(plans) + " plans"
        if !all, let scope { return p + " in " + scope }
        let j = projects == 1 ? "1 project" : String(projects) + " projects"
        return p + " · " + j
    }
}
