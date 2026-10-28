import CapstanAppCore
import CapstanKit
import SwiftUI

/// Hosts: the host list and, per host, a table of its projects with the state of each and what it means.
struct HostsScreen: View {
    let model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[4]) {
                HostsListView(model: model)
                ForEach(model.hosts) { host in
                    ConnectionBanners(model: model, hostId: host.id)
                    projects(host)
                }
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .navigationTitle("Hosts")
    }

    private func projects(_ host: HostConfig) -> some View {
        let list = HostLabel.projects(model.hostModels[host.id])
        return VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: HostLabel.hostName(host)).font(.system(size: 13, weight: .semibold))
            if list.isEmpty {
                Text("No projects read yet.").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            ForEach(list, id: \.projectId) { project in
                projectRow(host, project)
            }
        }
    }

    private func projectRow(_ host: HostConfig, _ project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        let focus = HostsFocus(hostId: host.id, projectId: project.projectId)
        return Button {
            model.hostsFocus = focus
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[2]) {
                Text(verbatim: HostLabel.projectName(project)).font(.system(size: 13)).frame(width: 140, alignment: .leading)
                StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
                Text(verbatim: text.meaning).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(Tokens.space[1])
            .background(
                model.hostsFocus == focus ? Tokens.color(.fill2) : Tokens.color(.fill),
                in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

#Preview("Hosts screen") {
    HostsScreen(model: AppModel(preview: PreviewData.standard)).frame(width: 760, height: 620)
}

#Preview("Hosts screen: stopped") {
    HostsScreen(model: AppModel(preview: HostsPreviewFixtures.seed(
        connection: .stopped(.cstanMissing(tail: UntrustedText(raw: "not found", verified: true, truncated: false))))))
        .frame(width: 760, height: 620)
}
