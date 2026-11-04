import CapstanAppCore
import CapstanKit
import SwiftUI

/// The rows of the sidebar. Agents and Plans stand for every project; the project in view is chosen in the toolbar.
private enum NavItem: Hashable {
    case waiting, agents, plans, messages, hosts
    case host(UUID)
    case project(hostId: UUID, projectId: String)
}

/// The aggregates across hosts (Waiting for you, Agents, Plans, Messages to PM, Hosts), then each host with its
/// projects. The selection is drawn here, in the accent, not by the system list, so no system blue shows.
struct SidebarView: View {
    let model: AppModel

    /// The hosts whose projects are folded away.
    @State private var folded: Set<UUID> = []

    /// Agents that need attention, in every project.
    private var attentionCount: Int {
        AgentShow.attention(AgentGroup.make(hosts: model.hosts, hostModels: model.hostModels)).count
    }

    /// Plans with an integration item that waits for the person.
    private var needsYouPlans: Int {
        PlanGroup.make(hosts: model.hosts, hostModels: model.hostModels)
            .reduce(0) { total, group in total + group.rows.filter { $0.hasUnintegratedWork && group.waiting($0) > 0 }.count }
    }

    /// Questions from agents that wait for an answer.
    private var openQuestions: Int {
        model.conversations().reduce(0) { $0 + $1.waiting }
    }

    var body: some View {
        let counts = model.sidebarCounts
        let current = item(for: model.selection)
        List {
            nav("Waiting for you", symbol: "tray", count: counts.waiting, item: .waiting, current: current)
            nav("Agents", symbol: "person.2", count: attentionCount, item: .agents, current: current)
            nav("Plans", symbol: "doc.text", count: needsYouPlans, item: .plans, current: current)
            nav("Messages to PM", symbol: "bubble.left", count: openQuestions, item: .messages, current: current)
            nav("Hosts", symbol: "server.rack", count: model.hosts.count, item: .hosts, current: current)
            Section {
                ForEach(model.hosts) { host in
                    hostRow(host, current: current)
                    if !folded.contains(host.id) {
                        let shown = HostLabel.projects(model.hostModels[host.id])
                        ForEach(shown.filter { $0.availability != .gone }, id: \.projectId) { project in
                            projectRow(host: host, project: project, counts: counts, current: current)
                        }
                        let gone = shown.filter { $0.availability == .gone }.count
                        if gone > 0 { goneRow(host: host, count: gone, current: current) }
                    }
                }
            } header: {
                Text("Hosts")
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .tint(Tokens.color(.accent))
    }

    /// One selectable row: a button whose background is the accent when it is the current one.
    private func row<Content: View>(
        _ item: NavItem, current: NavItem, @ViewBuilder _ content: () -> Content
    ) -> some View {
        let selected = item == current
        return Button {
            select(item)
        } label: {
            content()
                .padding(.vertical, Tokens.space[0])
                .padding(.horizontal, Tokens.space[1])
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: Tokens.radius.control)
                        .fill(selected ? Tokens.color(.accent).opacity(0.18) : Color.clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowInsets(EdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8))
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func nav(_ title: String, symbol: String, count: Int, item: NavItem, current: NavItem) -> some View {
        row(item, current: current) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: symbol)
                    .foregroundStyle(item == current ? Tokens.color(.accent) : Tokens.color(.label2))
                    .frame(width: 18)
                Text(verbatim: title)
                Spacer(minLength: Tokens.space[0])
                if count > 0 { Count(value: count, selected: item == current) }
            }
        }
    }

    private func hostRow(_ host: HostConfig, current: NavItem) -> some View {
        let projects: [ProjectModel] = model.hostModels[host.id].map { Array($0.projects.values) } ?? []
        let status = SidebarText.hostStatus(state: model.connection[host.id], projects: projects)
        let isFolded = folded.contains(host.id)
        return row(.host(host.id), current: current) {
            HStack(spacing: Tokens.space[1]) {
                Button {
                    if isFolded { folded.remove(host.id) } else { folded.insert(host.id) }
                } label: {
                    Image(systemName: isFolded ? "chevron.right" : "chevron.down")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(Tokens.color(.label2))
                        .frame(width: 12, height: 18)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(verbatim: isFolded ? "Show projects" : "Hide projects")) // static-text
                Circle()
                    .fill(status.tone == .green ? status.tone.color : Color.clear)
                    .overlay(Circle().strokeBorder(status.tone.color, lineWidth: status.tone == .green ? 0 : 1.5))
                    .frame(width: 8, height: 8)
                    .frame(width: 18)
                Text(verbatim: HostLabel.hostName(host)).lineLimit(1)
                Spacer(minLength: Tokens.space[0])
                Text(verbatim: status.word).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    private func projectRow(host: HostConfig, project: ProjectModel, counts: SidebarCounts, current: NavItem) -> some View {
        let waiting = counts.perProject[ProjectKey(hostId: host.id, projectId: project.projectId)] ?? 0
        let item = NavItem.project(hostId: host.id, projectId: project.projectId)
        return row(item, current: current) {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: HostLabel.projectName(project)).lineLimit(1)
                Spacer(minLength: Tokens.space[0])
                if let word = SidebarText.projectStatus(project.availability) {
                    Text(verbatim: word).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                } else if waiting > 0 {
                    Count(value: waiting, selected: item == current)
                }
            }
            .padding(.leading, Tokens.space[3])
        }
        .contextMenu {
            if ProjectRemovalPrompt.isOffered(project.availability) {
                Button(ProjectRemovalPrompt.menuTitle) { // static-text
                    model.projectToRemove = ProjectKey(hostId: host.id, projectId: project.projectId)
                }
            }
        }
    }

    /// Projects that are gone are one quiet row at the end of the host; it opens the host's table.
    private func goneRow(host: HostConfig, count: Int, current: NavItem) -> some View {
        row(.host(host.id), current: .hosts) {
            HStack {
                Text(verbatim: "\(count) gone").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                Spacer(minLength: Tokens.space[0])
            }
            .padding(.leading, Tokens.space[3])
        }
    }

    private func item(for selection: SidebarSelection) -> NavItem {
        switch selection {
        case .inbox: return .waiting
        case .messages: return .messages
        case .project(let hostId, let projectId): return .project(hostId: hostId, projectId: projectId)
        case .agents, .allAgents: return .agents
        case .plans, .allPlans: return .plans
        case .hosts: return model.hostsFocus.map { NavItem.host($0.hostId) } ?? NavItem.hosts
        }
    }

    private func select(_ item: NavItem) {
        switch item {
        case .waiting:
            model.inboxKind = nil
            model.selection = .inbox
        case .messages:
            model.selection = .messages
        case .agents:
            model.selection = .allAgents
        case .plans:
            model.selection = .allPlans
        case .hosts:
            model.hostsFocus = nil
            model.selection = .hosts
        case .host(let hostId):
            model.hostsFocus = HostsFocus(hostId: hostId)
            model.selection = .hosts
        case .project(let hostId, let projectId):
            model.inboxKind = nil
            model.selection = .project(hostId: hostId, projectId: projectId)
        }
    }

    /// The project Agents and Plans open on: the one in view, else the first one that has something to show.
    private func workProject() -> ProjectKey? {
        switch model.selection {
        case .project(let hostId, let projectId), .agents(let hostId, let projectId), .plans(let hostId, let projectId):
            return ProjectKey(hostId: hostId, projectId: projectId)
        case .inbox, .hosts, .messages, .allAgents, .allPlans:
            let keys = model.hosts.flatMap { host in
                HostLabel.projects(model.hostModels[host.id]).map { ProjectKey(hostId: host.id, projectId: $0.projectId) }
            }
            return keys.first { model.project($0.hostId, $0.projectId)?.availability == .ready } ?? keys.first
        }
    }
}

/// A count at the right of a row: small, tabular, quiet; in the accent on the selected row.
private struct Count: View {
    let value: Int
    var selected = false

    var body: some View {
        Text(verbatim: String(value))
            .font(.system(size: 12).monospacedDigit())
            .foregroundStyle(selected ? Tokens.color(.accent) : Tokens.color(.label2))
    }
}

#Preview("Sidebar: every availability") {
    SidebarView(model: AppModel(preview: PreviewData.standard))
        .frame(width: 248, height: 560)
}

#Preview("Sidebar: no hosts") {
    SidebarView(model: AppModel(preview: PreviewSeed()))
        .frame(width: 248, height: 320)
}
