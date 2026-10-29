import CapstanAppCore
import CapstanKit
import SwiftUI

/// The plans of every project with a progress bar and the counts in words, grouped by project.
struct PlansScreen: View {
    let model: AppModel

    var body: some View {
        let groups = projectGroups()
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                legend
                if groups.isEmpty {
                    Text("No plans yet").foregroundStyle(Tokens.color(.label2))
                }
                ForEach(groups, id: \.key) { group in
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        Text(verbatim: group.name)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(Tokens.color(.label2))
                        ForEach(group.rows, id: \.planId) { row in
                            planRow(row, key: group.key)
                        }
                    }
                }
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private struct ProjectPlans {
        let key: ProjectKey
        let name: String
        let rows: [PlanRow]
    }

    private func projectGroups() -> [ProjectPlans] {
        model.hosts.flatMap { host -> [ProjectPlans] in
            HostLabel.projects(model.hostModels[host.id]).compactMap { project -> ProjectPlans? in
                let rows = PlanRow.rows(project).filter(model.planFilter.matches)
                guard !rows.isEmpty else { return nil }
                return ProjectPlans(
                    key: ProjectKey(hostId: host.id, projectId: project.projectId),
                    name: HostLabel.projectName(project), rows: rows)
            }
        }
    }

    private var legend: some View {
        HStack(spacing: Tokens.space[2]) {
            swatch(Tokens.color(.green), "integrated")
            swatch(Tokens.color(.blue), "reported")
            swatch(Tokens.color(.label2), "assigned")
            swatch(Tokens.color(.fill2), "not started")
        }
        .font(.system(size: 12))
        .foregroundStyle(Tokens.color(.label2))
    }

    private func swatch(_ color: Color, _ word: String) -> some View {
        HStack(spacing: Tokens.space[0]) {
            RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 10, height: 10)
            Text(verbatim: word)
        }
    }

    private func planRow(_ row: PlanRow, key: ProjectKey) -> some View {
        let ref = PlanRef(hostId: key.hostId, projectId: key.projectId, planId: row.planId)
        let selected = model.selectedPlan == ref
        let waiting = model.project(key.hostId, key.projectId).map(IntegrationLink.waitingCount) ?? 0
        return Button {
            model.selectedPlan = ref
        } label: {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack {
                    Text(verbatim: "plan-\(row.sequence) · " + row.title)
                        .font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    Spacer()
                    if row.hasUnintegratedWork, waiting > 0 {
                        StatePill(word: "\(waiting) waits for you", symbol: "hand.raised", tone: .yellow)
                    } else {
                        StatePill(word: row.state.replacingOccurrences(of: "_", with: " ").capitalized, symbol: "circle", tone: .grey)
                    }
                }
                Text(verbatim: row.state + " · " + row.words).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                PlanProgressBar(progress: row.progress)
            }
            .padding(Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: Tokens.radius.row)
                    .fill(selected ? Tokens.color(.accent).opacity(0.16) : Color.clear))
            .overlay(
                RoundedRectangle(cornerRadius: Tokens.radius.row)
                    .strokeBorder(Tokens.color(.accent), lineWidth: selected ? 1 : 0))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Four stages in one bar: integrated, reported, assigned, not started.
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
        Rectangle().fill(color).frame(width: width * share)
    }
}

/// The plan the Plans screen selected: its stage, its counts, and the way to its agents and its waiting item.
struct PlanInspector: View {
    let model: AppModel

    var body: some View {
        if let ref = model.selectedPlan, let project = model.project(ref.hostId, ref.projectId),
            let row = PlanRow.rows(project).first(where: { $0.planId == ref.planId })
        {
            let waiting = IntegrationLink.waitingCount(project)
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                ScrollView {
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        Text(verbatim: "Plan · " + HostLabel.projectName(project) + " · " + row.state)
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                        Text(verbatim: "plan-\(row.sequence) · " + row.title).font(.system(size: 20, weight: .semibold))
                        PlanProgressBar(progress: row.progress)
                        Text(verbatim: row.words).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                }
                HStack {
                    Spacer()
                    Button {
                        model.selection = .agents(hostId: ref.hostId, projectId: ref.projectId)
                    } label: { Text("Show agents") }
                    .buttonStyle(.capstanQuiet)
                    if row.hasUnintegratedWork, waiting > 0 {
                        Button {
                            model.showProject(hostId: ref.hostId, projectId: ref.projectId, kind: .integration)
                        } label: { Text("Open the waiting item") }
                        .buttonStyle(.capstanPrimary)
                    }
                }
            }
            .padding(Tokens.space[3])
        } else {
            Text("Select a plan to see its progress")
                .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

#Preview("Plans: several progress points") {
    PlansScreen(model: AppModel(preview: PreviewData.standard))
        .frame(width: 520, height: 420)
}
