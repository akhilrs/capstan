import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Plans tab (board 08): the plans of every ready project, grouped by project, each with its id, a progress bar
/// by state with the same counts in words, and a pill when it waits for you, is in review or is ready to merge.
struct PlansScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let groups = model.planGroups()
        let count = groups.reduce(0) { $0 + $1.rows.count }
        List {
            Section {
                Text(verbatim: summary(count, projects: groups.count))
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                    .listRowBackground(Color.clear)
            }
            if groups.isEmpty {
                Section {
                    ContentUnavailableView(
                        "No plans", systemImage: "list.bullet.rectangle",
                        description: Text("The plans of your ready projects appear here."))
                }
                .listRowBackground(Color.clear)
            }
            ForEach(groups) { group in
                Section {
                    ForEach(group.rows, id: \.planId) { row in
                        NavigationLink(
                            value: PhoneRoute.plan(PlanRef(hostId: group.hostId, projectId: group.projectId, planId: row.planId))
                        ) {
                            PlanCard(row: row, waiting: group.waiting(row))
                        }
                    }
                } header: {
                    Text(verbatim: group.projectName + " · " + group.hostName)
                        .font(TypeStyle.footnote.weight(.semibold))
                }
            }
            if !groups.isEmpty {
                Section {
                } footer: {
                    Text("The bar: green integrated, blue reported, grey assigned, empty not started.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Tokens.color(.ground))
        .navigationTitle("Plans")
        .navigationBarTitleDisplayMode(.large)
    }

    private func summary(_ plans: Int, projects: Int) -> String {
        let p = plans == 1 ? "1 plan" : String(plans) + " plans"
        let j = projects == 1 ? "1 project" : String(projects) + " projects"
        return p + " · " + j
    }
}

/// The plans of one project.
struct PlanGroup: Identifiable {
    let hostId: UUID
    let projectId: String
    let projectName: String
    let hostName: String
    let rows: [PlanRow]
    let project: ProjectModel
    var id: String { hostId.uuidString + "/" + projectId }

    /// How many integration items of the plan wait for you.
    func waiting(_ row: PlanRow) -> Int {
        IntegrationLink.waiting(for: PlanRow.plan(for: row, in: project), in: project).count
    }
}

extension AppModel {
    /// The plans of every ready project, by host name and then project name.
    func planGroups() -> [PlanGroup] {
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
}

/// A plan in the list: its title and id, its bar, the meta line, and a pill when it means something.
struct PlanCard: View {
    let row: PlanRow
    let waiting: Int

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: row.title).font(TypeStyle.headline).lineLimit(2)
                Spacer(minLength: Tokens.space[1])
                Text(verbatim: row.planId).font(TypeStyle.monoSmall).foregroundStyle(Tokens.color(.label2))
            }
            PlanProgressBar(progress: row.progress)
                .accessibilityHidden(true)
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: row.metaLine)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(2)
                Spacer(minLength: 0)
                if let pill = row.pill(waiting: waiting) {
                    StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
                }
            }
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }
}

/// Four stages in one bar: integrated, reported, assigned, not started. The words beside it say the same counts.
struct PlanProgressBar: View {
    let progress: PlanProgress

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: 0) {
                segment(progress.integrated, Tokens.color(.green), geo.size.width)
                segment(progress.reported, Tokens.color(.blue), geo.size.width)
                segment(progress.assigned, Tokens.color(.label2), geo.size.width)
                Spacer(minLength: 0)
            }
        }
        .frame(height: 6)
        .background(Tokens.color(.fill2), in: Capsule())
        .clipShape(Capsule())
    }

    private func segment(_ share: Double, _ color: Color, _ width: CGFloat) -> some View {
        Rectangle().fill(color).frame(width: max(0, width * share))
    }
}
