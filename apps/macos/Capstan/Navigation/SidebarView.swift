import CapstanAppCore
import CapstanKit
import SwiftUI

/// The rows of the sidebar. Agents and Plans stand for every project; the project in view is chosen in the toolbar.
private enum NavItem: Hashable {
    case waiting, agents, plans, messages, hosts
    case host(UUID)
    case project(hostId: UUID, projectId: String)
}

/// The aggregates across hosts (Waiting for you, Agents, Plans, Messages to PM), then each host with its projects.
struct SidebarView: View {
    let model: AppModel

    var body: some View {
        let counts = model.sidebarCounts
        List(selection: selection) {
            nav("Waiting for you", symbol: "tray", count: counts.waiting, item: .waiting)
            nav("Agents", symbol: "person.2", count: counts.agents, item: .agents)
            nav("Plans", symbol: "doc.text", count: counts.plans, item: .plans)
            nav("Messages to PM", symbol: "bubble.left", count: counts.messages, item: .messages)
            nav("Hosts", symbol: "server.rack", count: model.hosts.count, item: .hosts)
            Section {
                ForEach(model.hosts) { host in
                    hostRow(host)
                    ForEach(HostLabel.projects(model.hostModels[host.id]), id: \.projectId) { project in
                        projectRow(host: host, project: project, counts: counts)
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

    private func nav(_ title: String, symbol: String, count: Int, item: NavItem) -> some View {
        HStack(spacing: Tokens.space[1]) {
            Label(title, systemImage: symbol)
            Spacer(minLength: Tokens.space[0])
            if count > 0 { Count(value: count) }
        }
        .tag(item)
        .accessibilityElement(children: .combine)
    }

    private func hostRow(_ host: HostConfig) -> some View {
        let projects: [ProjectModel] = model.hostModels[host.id].map { Array($0.projects.values) } ?? []
        let status = SidebarText.hostStatus(state: model.connection[host.id], projects: projects)
        return HStack(spacing: Tokens.space[1]) {
            Circle()
                .fill(status.tone == .green ? status.tone.color : Color.clear)
                .overlay(Circle().strokeBorder(status.tone.color, lineWidth: status.tone == .green ? 0 : 1.5))
                .frame(width: 8, height: 8)
            Text(verbatim: HostLabel.hostName(host)).lineLimit(1)
            Spacer(minLength: Tokens.space[0])
            Text(verbatim: status.word).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        }
        .tag(NavItem.host(host.id))
        .accessibilityElement(children: .combine)
    }

    private func projectRow(host: HostConfig, project: ProjectModel, counts: SidebarCounts) -> some View {
        let waiting = counts.perProject[ProjectKey(hostId: host.id, projectId: project.projectId)] ?? 0
        return HStack(spacing: Tokens.space[1]) {
            Text(verbatim: HostLabel.projectName(project)).lineLimit(1)
            Spacer(minLength: Tokens.space[0])
            if let word = SidebarText.projectStatus(project.availability) {
                Text(verbatim: word).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            } else if waiting > 0 {
                Count(value: waiting)
            }
        }
        .padding(.leading, Tokens.space[3])
        .tag(NavItem.project(hostId: host.id, projectId: project.projectId))
        .accessibilityElement(children: .combine)
    }

    private var selection: Binding<NavItem?> {
        Binding(
            get: { item(for: model.selection) },
            set: { if let new = $0 { select(new) } })
    }

    private func item(for selection: SidebarSelection) -> NavItem {
        switch selection {
        case .inbox: return model.inboxKind == .message ? .messages : .waiting
        case .project(let hostId, let projectId): return .project(hostId: hostId, projectId: projectId)
        case .agents: return .agents
        case .plans: return .plans
        case .hosts:
            return model.hostsFocus.map { NavItem.host($0.hostId) } ?? NavItem.hosts
        }
    }

    private func select(_ item: NavItem) {
        switch item {
        case .waiting:
            model.inboxKind = nil
            model.selection = .inbox
        case .messages:
            model.inboxKind = .message
            model.selection = .inbox
        case .agents:
            if let key = workProject() { model.selection = .agents(hostId: key.hostId, projectId: key.projectId) }
        case .plans:
            if let key = workProject() { model.selection = .plans(hostId: key.hostId, projectId: key.projectId) }
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
        case .inbox, .hosts:
            let keys = model.hosts.flatMap { host in
                HostLabel.projects(model.hostModels[host.id]).map { ProjectKey(hostId: host.id, projectId: $0.projectId) }
            }
            return keys.first { model.project($0.hostId, $0.projectId)?.availability == .ready } ?? keys.first
        }
    }
}

/// A count at the right of a row: small, tabular, quiet.
private struct Count: View {
    let value: Int

    var body: some View {
        Text(verbatim: String(value))
            .font(.system(size: 12).monospacedDigit())
            .foregroundStyle(Tokens.color(.label2))
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
