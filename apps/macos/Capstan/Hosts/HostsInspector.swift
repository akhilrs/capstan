import CapstanAppCore
import CapstanKit
import SwiftUI

/// The host or project the Hosts screen has in focus: what it is, what its state means and how to fix it.
struct HostsInspector: View {
    let model: AppModel
    @State private var removing: HostConfig?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                if let focus = model.hostsFocus, let host = model.hosts.first(where: { $0.id == focus.hostId }) {
                    if let id = focus.projectId, let project = model.project(host.id, id) {
                        projectSection(host, project)
                    } else {
                        hostSection(host)
                    }
                } else {
                    Text("Select a host or a project.")
                        .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                }
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .confirmationDialog(
            "Remove this host?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            presenting: removing
        ) { host in
            Button("Remove", role: .destructive) { model.removeHost(host.id) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("The app forgets the host and what it saved of it. The host itself is not changed.")
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
        Text(verbatim: "Host").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        Text(verbatim: HostLabel.hostName(host)).font(.system(size: 20, weight: .semibold))
        HostLabel.pill(model.connection[host.id])
        MetaList(rows: rows(host))
        HStack {
            Button("Remove host…") { removing = host }.buttonStyle(.capstanQuiet)
            Spacer()
            Button("Edit host…") {
                if model.presented == nil { model.presented = .addHost(editing: host) }
            }
            .buttonStyle(.capstanPrimary)
        }
    }

    @ViewBuilder
    private func projectSection(_ host: HostConfig, _ project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        HStack(spacing: Tokens.space[1]) {
            StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
            Text(verbatim: HostLabel.projectName(project) + " on " + HostLabel.hostName(host))
                .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        }
        Text(verbatim: text.meaning).font(.system(size: 20, weight: .semibold))
        MetaList(rows: [
            MetaRow(label: "Root", value: plain(project.root)),
            MetaRow(label: "Wire state", value: plain(project.availability.wireName)),
        ])
        if project.availability != .ready {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Label("How to fix it", systemImage: "wrench.and.screwdriver").font(.system(size: 12, weight: .semibold))
                Text(verbatim: text.fix).font(.system(size: 13))
            }
            .padding(Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            HStack {
                Spacer()
                Button("Check again") { model.refresh(hostId: host.id, projectIds: [project.projectId]) }
                    .buttonStyle(.capstanPrimary)
            }
        }
        otherStates(except: project.availability)
    }

    /// The states a project can be in, with what each means.
    private func otherStates(except current: ProjectAvailability) -> some View {
        let states: [ProjectAvailability] = [.silent, .refused, .unsupported, .gone, .noKey].filter { $0 != current }
        return VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text("The other states").font(.system(size: 12, weight: .semibold)).foregroundStyle(Tokens.color(.label2))
            Divider()
            Grid(alignment: .topLeading, horizontalSpacing: Tokens.space[2], verticalSpacing: Tokens.space[1]) {
                ForEach(states, id: \.wireName) { state in
                    let text = AvailabilityText(state)
                    GridRow {
                        Text(verbatim: text.word).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                        Text(verbatim: text.meaning + " " + text.fix).font(.system(size: 12))
                    }
                }
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
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
