import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Hosts tab (board 12): one section per host with its state and age in the header, why it is not live with
/// the fix, and its projects with their state as a word and glyph. The header opens the host's details; a project
/// opens its overview or, when it is not ready, its state and fix.
struct HostsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if model.hosts.isEmpty {
                ContentUnavailableView {
                    Label("No host yet", systemImage: "server.rack")
                } description: {
                    Text("Add a host that runs Capstan to follow its projects.")
                } actions: {
                    Button("Add a host") { model.navigation.sheet = .addHost }
                        .buttonStyle(.capstanPrimary)
                        .fixedSize()
                }
            } else {
                List {
                    ForEach(model.hosts, id: \.id) { host in
                        hostSection(host)
                    }
                    Section {
                        NavigationLink(value: PhoneRoute.settings) {
                            Label("Settings", systemImage: "gearshape")
                        }
                    }
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
                .refreshable { refreshAll() }
            }
        }
        .background(Tokens.color(.ground))
        .navigationTitle("Hosts")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { openSettings() } label: {
                    Label("Settings", systemImage: "gearshape")
                }
                Button { model.navigation.sheet = .addHost } label: {
                    Label("Add a host", systemImage: "plus")
                }
            }
        }
    }

    private func openSettings() {
        model.navigation.show(.hosts, model.navigation.path(.hosts) + [.settings])
    }

    private func hostSection(_ host: HostConfig) -> some View {
        let projects = (model.hostModels[host.id]?.projects.values).map { Array($0) } ?? []
        let sorted = projects.sorted { $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending }
        return Section {
            HostBanners(host: host)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: Tokens.space[1], leading: 0, bottom: Tokens.space[1], trailing: 0))
            if sorted.isEmpty {
                Text("No projects yet")
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
            }
            ForEach(sorted, id: \.projectId) { project in
                NavigationLink(value: PhoneRoute.project(hostId: host.id, projectId: project.projectId)) {
                    ProjectRow(project: project)
                }
            }
        } header: {
            HostSectionHeader(host: host)
        }
    }

    private func refreshAll() {
        for host in model.hosts {
            model.refresh(hostId: host.id, projectIds: model.readyProjects(hostId: host.id))
        }
    }
}

/// The header of a host's section: a dot and "name · state" on the left, the heartbeat age or the retry time on the
/// right. It opens the host's details.
struct HostSectionHeader: View {
    @Environment(AppModel.self) private var model
    let host: HostConfig

    var body: some View {
        Button {
            model.navigation.show(.hosts, model.navigation.path(.hosts) + [.host(host.id)])
        } label: {
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let header = HostStatusText.header(
                    model.connection[host.id], problem: model.problems[host.id], now: context.date)
                HStack(spacing: Tokens.space[1]) {
                    Circle()
                        .fill(HostStatusText.color(header.tone))
                        .frame(width: 8, height: 8)
                        .accessibilityHidden(true)
                    Text(verbatim: host.displayName + " · " + header.word)
                        .font(TypeStyle.footnote.weight(.semibold))
                        .foregroundStyle(Tokens.color(.label2))
                    Spacer(minLength: Tokens.space[1])
                    if let trailing = header.trailing {
                        Text(verbatim: trailing)
                            .font(TypeStyle.footnote.monospacedDigit())
                            .foregroundStyle(Tokens.color(.label2))
                    }
                    Image(systemName: "info.circle")
                        .foregroundStyle(Tokens.color(.accent))
                        .accessibilityHidden(true)
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
        }
        .buttonStyle(.plain)
        .textCase(nil)
        .accessibilityHint("Shows the host's details")
    }
}

/// A project of a host: its name, its state as a pill with a word and glyph, and either what waits (ready) or
/// what the state means.
struct ProjectRow: View {
    let project: ProjectModel
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let text = AvailabilityText(project.availability)
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: Tokens.space[1]))
            : AnyLayout(HStackLayout(alignment: .firstTextBaseline))
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            layout {
                Text(verbatim: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text)
                    .font(TypeStyle.headline)
                    .lineLimit(typeSize.isAccessibilitySize ? 3 : 1)
                if !typeSize.isAccessibilitySize { Spacer(minLength: Tokens.space[1]) }
                StatePill(word: text.word, symbol: text.symbol, tone: AvailabilityText.tone(project.availability))
            }
            Text(verbatim: line(text))
                .font(TypeStyle.subheadline)
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(2)
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }

    private func line(_ text: AvailabilityText) -> String {
        guard project.availability == .ready else { return text.meaning }
        if !project.snapshotComplete { return "Reading the project…" }
        let waits = project.waiting.count
        let agents = project.agents.count
        return String(waits) + " wait · " + (agents == 1 ? "1 agent" : String(agents) + " agents")
    }
}

#Preview("Hosts") {
    NavigationStack { HostsScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Hosts, problems") {
    NavigationStack { HostsScreen().routes() }
        .environment(AppModel(preview: PreviewData.problems))
}

#Preview("Hosts, empty") {
    NavigationStack { HostsScreen() }
        .environment(AppModel(preview: PreviewData.empty))
}

#Preview("Hosts, dark") {
    NavigationStack { HostsScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
        .preferredColorScheme(.dark)
}
