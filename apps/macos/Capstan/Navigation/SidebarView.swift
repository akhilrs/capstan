import CapstanAppCore
import CapstanKit
import SwiftUI

/// Waiting for you, then each host with its projects and their Agents and Plans, then Hosts.
struct SidebarView: View {
    let model: AppModel

    var body: some View {
        List(selection: selection) {
            Label("Waiting for you", systemImage: "tray.full")
                .badge(model.badgeCount)
                .tag(SidebarSelection.inbox)
            ForEach(model.hosts) { host in
                Section {
                    ForEach(HostLabel.projects(model.hostModels[host.id]), id: \.projectId) { project in
                        projectRows(host: host, project: project)
                    }
                } header: {
                    Text(verbatim: HostLabel.hostName(host))
                }
            }
            Section {
                Label("Hosts", systemImage: "server.rack")
                    .tag(SidebarSelection.hosts)
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
    }

    @ViewBuilder
    private func projectRows(host: HostConfig, project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        HStack(spacing: Tokens.space[1]) {
            Image(systemName: "folder")
            Text(verbatim: HostLabel.projectName(project)).lineLimit(1)
            Spacer(minLength: Tokens.space[0])
            if project.availability != .ready {
                Image(systemName: text.symbol).foregroundStyle(HostLabel.tone(project.availability).color)
                Text(verbatim: text.word).font(.system(size: 11)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .tag(SidebarSelection.project(hostId: host.id, projectId: project.projectId))
        .accessibilityElement(children: .combine)
        Label("Agents", systemImage: "person.2")
            .padding(.leading, Tokens.space[3])
            .tag(SidebarSelection.agents(hostId: host.id, projectId: project.projectId))
        Label("Plans", systemImage: "list.bullet.rectangle")
            .padding(.leading, Tokens.space[3])
            .tag(SidebarSelection.plans(hostId: host.id, projectId: project.projectId))
    }

    private var selection: Binding<SidebarSelection?> {
        Binding(
            get: { model.selection },
            set: { if let new = $0 { model.selection = new } })
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
