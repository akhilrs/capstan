import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Agents tab (board 07): every agent of every ready project, grouped by project, with a segmented filter (All,
/// Needs attention, Paused). A row shows the agent, its role, its health as a word with a glyph and how long, and
/// its task and branch. A row opens the agent's details.
struct AgentsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var filter: AgentFilter = .all

    var body: some View {
        let groups = model.agentGroups()
        let all = groups.flatMap { $0.rows }
        List {
            Section {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    Text(verbatim: summary(all.count, projects: groups.count))
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                    Picker(selection: $filter) {
                        ForEach(AgentFilter.allCases, id: \.self) { option in
                            Text(verbatim: AgentsScreen.title(option) + " " + String(option.count(all))).tag(option)
                        }
                    } label: {
                        Text("Show")
                    }
                    .pickerStyle(.segmented)
                }
                .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
            if all.isEmpty {
                Section {
                    ContentUnavailableView(
                        "No agents", systemImage: "person.2",
                        description: Text("The agents of your ready projects appear here."))
                }
                .listRowBackground(Color.clear)
            }
            ForEach(groups) { group in
                let rows = group.rows.filter(filter.matches)
                if !rows.isEmpty {
                    Section {
                        ForEach(rows, id: \.source.key) { row in
                            NavigationLink(
                                value: PhoneRoute.agent(hostId: group.hostId, projectId: group.projectId, agentId: row.source.agentId)
                            ) {
                                AgentListRow(row: row)
                            }
                        }
                    } header: {
                        HStack {
                            Text(verbatim: group.projectName + " · " + group.hostName)
                            Spacer()
                            Text(verbatim: String(rows.count))
                        }
                        .font(TypeStyle.footnote.weight(.semibold))
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Tokens.color(.ground))
        .navigationTitle("Agents")
        .navigationBarTitleDisplayMode(.large)
        .onChange(of: filter) { _, _ in Haptics.play(.selection) }
    }

    private func summary(_ agents: Int, projects: Int) -> String {
        let a = agents == 1 ? "1 agent" : String(agents) + " agents"
        let p = projects == 1 ? "1 project" : String(projects) + " projects"
        return a + " · " + p
    }

    static func title(_ filter: AgentFilter) -> String {
        switch filter {
        case .all: return "All"
        case .needAttention: return "Needs attention"
        case .paused: return "Paused"
        }
    }
}

/// The agents of one project, as the Agents tab groups them.
struct AgentGroup: Identifiable {
    let hostId: UUID
    let projectId: String
    let projectName: String
    let hostName: String
    let rows: [AgentRow]
    var id: String { hostId.uuidString + "/" + projectId }
}

extension AppModel {
    /// The agents of every ready project, by host name and then project name.
    func agentGroups() -> [AgentGroup] {
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
}
