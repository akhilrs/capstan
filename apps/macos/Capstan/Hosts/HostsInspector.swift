import CapstanAppCore
import CapstanKit
import SwiftUI

/// The host or project the Hosts screen has in focus: what it is, what its state means and how to fix it.
struct HostsInspector: View {
    let model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                if let focus = model.hostsFocus, let host = model.hosts.first(where: { $0.id == focus.hostId }) {
                    hostSection(host)
                    if let id = focus.projectId, let project = model.project(host.id, id) {
                        projectSection(project)
                    }
                } else {
                    Text("Select a host or a project.")
                        .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                }
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func plain(_ s: String) -> UntrustedText {
        UntrustedText(raw: s, verified: true, truncated: false)
    }

    private func rows(_ host: HostConfig) -> [MetaRow] {
        var rows = [
            MetaRow(label: "Destination", value: plain(host.destination)),
            MetaRow(label: "cstan path", value: plain(host.cstanPath)),
            MetaRow(label: "Poll interval", value: plain(String(host.pollMs) + " ms")),
        ]
        if let hello = model.hostModels[host.id]?.hello {
            rows.append(MetaRow(label: "Host", value: hello.host))
            rows.append(MetaRow(label: "Capstan", value: hello.version))
        }
        return rows
    }

    @ViewBuilder
    private func hostSection(_ host: HostConfig) -> some View {
        Text(verbatim: HostLabel.hostName(host)).font(.system(size: 15, weight: .semibold))
        HostLabel.pill(model.connection[host.id])
        MetaList(rows: rows(host))
        Button("Edit host…") {
            if model.presented == nil { model.presented = .addHost(editing: host) }
        }
    }

    @ViewBuilder
    private func projectSection(_ project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        Divider()
        Text(verbatim: HostLabel.projectName(project)).font(.system(size: 15, weight: .semibold))
        StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
        Text(verbatim: text.meaning).font(.system(size: 13))
        Text(verbatim: "Fix: " + text.fix).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
        UntrustedTextBlock(text: plain(project.root), label: "Folder on the host")
    }
}

#Preview("Inspector: project") {
    let model = AppModel(preview: PreviewData.standard)
    model.hostsFocus = HostsFocus(hostId: PreviewData.alphaId, projectId: "notes")
    return HostsInspector(model: model).frame(width: 360, height: 480)
}

#Preview("Inspector: host") {
    let model = AppModel(preview: PreviewData.standard)
    model.hostsFocus = HostsFocus(hostId: PreviewData.betaId)
    return HostsInspector(model: model).frame(width: 360, height: 480)
}

#Preview("Inspector: nothing") {
    HostsInspector(model: AppModel(preview: PreviewSeed())).frame(width: 360, height: 200)
}
