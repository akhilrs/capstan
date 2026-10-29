import CapstanAppCore
import CapstanKit
import SwiftUI

/// Waiting for you: the items of every host and project, grouped by kind. The title, the filter and the search are
/// in the window toolbar.
struct InboxScreen: View {
    @Bindable var model: AppModel

    var body: some View {
        Group {
            if case .project(let hostId, let projectId) = model.selection,
                let project = model.project(hostId, projectId), project.availability != .ready, project.waiting.isEmpty
            {
                // Not ready and nothing to show: say why and what to do, never skeleton rows.
                ProjectStateView(model: model, hostId: hostId, project: project)
            } else {
                content
            }
        }
        .onChange(of: model.visibleRowIDs) { _, ids in pruneSelection(ids) }
        .onAppear { pruneSelection(model.visibleRowIDs) }
    }

    /// The inspector keeps a row only while the list still shows it.
    private func pruneSelection(_ ids: [InboxRowID]) {
        if let selected = model.selectedRow, !ids.contains(selected) { model.selectedRow = nil }
    }

    /// The compact banner above rows of a project that is not ready: what it means, and Check again.
    @ViewBuilder
    private var stateBanner: some View {
        if case .project(let hostId, let projectId) = model.selection,
            let project = model.project(hostId, projectId), project.availability != .ready
        {
            let text = AvailabilityText(project.availability)
            HStack(spacing: Tokens.space[1]) {
                StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
                Text(verbatim: text.fix).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(2)
                Spacer(minLength: Tokens.space[0])
                Button { model.refresh(hostId: hostId, projectIds: [projectId]) } label: { Text("Check again") }
                    .buttonStyle(.capstanQuiet)
            }
            .padding(Tokens.space[2])
            .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
            .padding(.horizontal, Tokens.space[3])
            .padding(.bottom, Tokens.space[1])
        }
    }

    private var content: some View {
        let groups = model.displayGroups()
        return VStack(alignment: .leading, spacing: 0) {
            stateBanner
            ForEach(model.hosts) { config in
                if !isLive(config.id) {
                    ConnectionBanners(model: model, hostId: config.id)
                        .padding(.horizontal, Tokens.space[3])
                        .padding(.bottom, Tokens.space[1])
                }
            }
            if groups.isEmpty {
                placeholder
            } else {
                list(groups)
            }
        }
        .searchable(text: $model.inboxSearch, placement: .toolbar, prompt: "Search")
    }

    private func isLive(_ hostId: UUID) -> Bool {
        if case .live? = model.connection[hostId] { return true }
        return false
    }

    /// True until the hosts that are connecting have sent their first snapshot.
    private var loading: Bool {
        model.hosts.contains { config in
            switch model.connection[config.id] {
            case nil, .connecting?: return true
            default:
                return model.hostModels[config.id]?.projects.values.contains { !$0.snapshotComplete && $0.availability == .ready } ?? false
            }
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        if loading {
            VStack(spacing: Tokens.space[2]) {
                ForEach(0..<3, id: \.self) { _ in skeletonRow }
            }
            .padding(Tokens.space[3])
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Loading")
            Spacer()
        } else {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Image(systemName: "tray").font(.system(size: 26)).foregroundStyle(Tokens.color(.label2))
                Text("Nothing is waiting for you").font(.system(size: 15, weight: .semibold))
                Text(verbatim: emptyDetail)
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
            }
            .padding(Tokens.space[5])
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }

    /// A row-shaped placeholder: a square for the glyph, two bars for the lines and a short one for the age.
    private var skeletonRow: some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            RoundedRectangle(cornerRadius: 4).fill(Tokens.color(.fill2)).frame(width: 14, height: 14)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Capsule().fill(Tokens.color(.fill2)).frame(width: 150, height: 8)
                Capsule().fill(Tokens.color(.fill2)).frame(maxWidth: 360, minHeight: 8, maxHeight: 8)
            }
            Spacer(minLength: Tokens.space[0])
            Capsule().fill(Tokens.color(.fill2)).frame(width: 36, height: 8)
        }
    }

    private var emptyDetail: String {
        let projects = model.hostModels.values.reduce(0) { $0 + $1.projects.count }
        let live = model.hosts.filter { isLive($0.id) }.count
        let scope = "\(projects) project\(projects == 1 ? "" : "s") on \(live) host\(live == 1 ? " is" : "s are") live."
        return scope + " You get a notification when an agent asks for something."
    }

    private func list(_ groups: [DisplayGroup]) -> some View {
        let ids = groups.flatMap { $0.rows.map(\.id) }
        // The selection is drawn by the row, in the accent: a system list would draw it in the system accent.
        return List {
            ForEach(groups) { group in
                Section {
                    ForEach(group.rows) { row in
                        Button {
                            model.selectedRow = row.id
                        } label: {
                            InboxRowView(
                                row: row, forwarded: model.forwardShown(row.id), selected: model.selectedRow == row.id,
                                quiet: group.quiet)
                        }
                        .buttonStyle(.plain)
                        .listRowInsets(EdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8))
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                        .accessibilityAddTraits(model.selectedRow == row.id ? .isSelected : [])
                    }
                } header: {
                    KindGroupHeader(title: group.header, quiet: group.quiet)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .tint(Tokens.color(.accent))
        .focusable()
        .onKeyPress(.downArrow) { move(ids, by: 1) }
        .onKeyPress(.upArrow) { move(ids, by: -1) }
        .onKeyPress(.return) {
            guard model.selectedRow == nil, let first = ids.first else { return .handled }
            model.selectedRow = first
            return .handled
        }
    }

    /// Moves the selection one row; the first press selects the first row.
    private func move(_ ids: [InboxRowID], by step: Int) -> KeyPress.Result {
        guard !ids.isEmpty else { return .ignored }
        guard let current = model.selectedRow, let index = ids.firstIndex(of: current) else {
            model.selectedRow = ids[step > 0 ? 0 : ids.count - 1]
            return .handled
        }
        model.selectedRow = ids[min(max(index + step, 0), ids.count - 1)]
        return .handled
    }
}

/// The empty view of a project that is not ready: what the state means and how to fix it (screen 07's wording).
private struct ProjectStateView: View {
    let model: AppModel
    let hostId: UUID
    let project: ProjectModel

    var body: some View {
        let text = AvailabilityText(project.availability)
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
            Text(verbatim: text.meaning).font(.system(size: 15, weight: .semibold))
            Text(verbatim: text.fix).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            Button { model.refresh(hostId: hostId, projectIds: [project.projectId]) } label: { Text("Check again") }
                .buttonStyle(.capstanPrimary)
                .padding(.top, Tokens.space[0])
        }
        .padding(Tokens.space[5])
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

#if DEBUG
#Preview("InboxScreen") {
    InboxScreen(model: InboxPreviewFixtures.model())
        .frame(width: 560, height: 640)
}

#Preview("InboxScreen, stale and no decide") {
    InboxScreen(model: InboxPreviewFixtures.model(decide: false, stale: true))
        .frame(width: 560, height: 640)
}

#Preview("InboxScreen, empty") {
    InboxScreen(model: InboxPreviewFixtures.emptyModel())
        .frame(width: 560, height: 480)
}
#endif
