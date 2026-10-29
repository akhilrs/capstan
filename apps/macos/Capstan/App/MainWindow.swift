import CapstanAppCore
import CapstanKit
import SwiftUI

/// Sidebar, content, inspector. Under `foldWidth` the sidebar folds and the inspector moves into a sheet.
struct MainWindow: View {
    @Bindable var model: AppModel
    @State private var columnVisibility = NavigationSplitViewVisibility.all
    @State private var narrow = false
    @State private var windowWidth: CGFloat = 1400
    @State private var inspectorSheet = false

    private static let foldWidth: CGFloat = 900
    /// The widths the toolbar needs beside the title: the whole filter, the search field and the gaps.
    private static let filterWidth: CGFloat = 340
    private static let searchWidth: CGFloat = 220
    private static let sidebarWidth: CGFloat = 248
    private static let inspectorWidth: CGFloat = 360

    /// Whether the content column is too narrow for the title, the whole filter and the search; the filter then folds
    /// into one menu, so no label is cut and nothing overlaps the title.
    private var compactToolbar: Bool {
        let titleWidth = CGFloat(titleText(compact: false).count) * 8.5 + 40
        let room = windowWidth - Self.sidebarWidth - Self.inspectorWidth
        return room < titleWidth + Self.filterWidth + Self.searchWidth + 48
    }
    init(model: AppModel) {
        self.model = model
    }

    var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            SidebarView(model: model)
                .navigationSplitViewColumnWidth(min: 248, ideal: 248, max: 248)
        } detail: {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .safeAreaInset(edge: .bottom, alignment: .leading) {
                    StatusCapsule(model: model)
                        .padding(Tokens.space[2])
                }
                .inspector(isPresented: inspectorBinding) {
                    inspector
                        .inspectorColumnWidth(min: 360, ideal: 360, max: 360)
                }
        }
        .sheet(isPresented: $inspectorSheet) {
            inspector
                .frame(minWidth: 360, minHeight: 320)
        }
        .sheet(item: $model.presented) { sheet in
            switch sheet {
            case .addHost(let editing):
                AddHostSheet(model: model, editing: editing) { config in
                    if let config {
                        if editing == nil { model.addHost(config) } else { model.updateHost(config) }
                    }
                    model.presented = nil
                }
            }
        }
        .toolbar {
            // One item for the title capsule and the filter capsule, so the spacing between them is fixed. Each is
            // glass of its own; the system's shared background is off, or it would draw one capsule around both.
            ToolbarItem(placement: .navigation) {
                HStack(spacing: Tokens.space[1]) {
                    Text(verbatim: title)
                        .font(.system(size: 15, weight: .semibold))
                        .lineLimit(1)
                        .fixedSize()
                        .padding(.horizontal, Tokens.space[2])
                        .padding(.vertical, Tokens.space[0] + 2)
                        .glassEffect(.regular, in: Capsule())
                    filterCapsule
                }
            }
            .sharedBackgroundVisibility(.hidden)
            ToolbarItem(placement: .primaryAction) {
                if case .hosts = model.selection {
                    Button {
                        if model.presented == nil { model.presented = .addHost(editing: nil) }
                    } label: {
                        Label { Text("Add host…") } icon: { Image(systemName: "square.and.pencil") }
                    }
                }
            }
        }
        .tint(Tokens.color(.accent))
        .accentColor(Tokens.color(.accent))
        .toolbar(removing: .title)
        .navigationTitle("")
        .frame(minWidth: 640, minHeight: 480)
        .onGeometryChange(for: CGFloat.self) { proxy in
            proxy.size.width
        } action: { width in
            Task { @MainActor in windowWidth = width }
        }
        .onGeometryChange(for: Bool.self) { proxy in
            proxy.size.width < Self.foldWidth
        } action: { isNarrow in
            // Written after the layout pass: a state change inside it makes SwiftUI relayout the view mid-layout.
            Task { @MainActor in
                narrow = isNarrow
                columnVisibility = isNarrow ? .detailOnly : .all
                if !isNarrow { inspectorSheet = false }
            }
        }
        .onChange(of: model.selectedRow) { _, row in
            if narrow && hasInspector && row != nil { inspectorSheet = true }
        }
        .registersWindowOpener(model)
    }

    @ViewBuilder
    private var content: some View {
        switch model.selection {
        case .inbox, .project:
            InboxScreen(model: model)
        case .agents(let hostId, let projectId):
            AgentsScreen(model: model, hostId: hostId, projectId: projectId)
        case .plans:
            PlansScreen(model: model)
        case .hosts:
            HostsScreen(model: model)
        case .messages:
            MessagesScreen()
        }
    }

    @ViewBuilder
    private var inspector: some View {
        switch model.selection {
        case .inbox, .project:
            InspectorScreen(model: model, row: model.selectedRow)
        case .hosts:
            HostsInspector(model: model)
        case .agents(let hostId, let projectId):
            AgentInspector(model: model, hostId: hostId, projectId: projectId)
        case .plans:
            PlanInspector(model: model)
        case .messages:
            EmptyView()
        }
    }

    private var hasInspector: Bool {
        switch model.selection {
        case .inbox, .project, .hosts, .agents, .plans: return true
        case .messages: return false
        }
    }

    /// Wide windows always show the inspector as a column, with its placeholder when nothing is selected; narrow ones never do.
    private var inspectorBinding: Binding<Bool> {
        Binding(
            get: { !narrow && hasInspector },
            // The system may ask to hide the column (a toolbar toggle, a selection change); it stays, as in the design.
            set: { _ in })
    }

    @ViewBuilder
    private var filterCapsule: some View {
        switch model.selection {
        case .inbox, .project:
            KindFilter(model: model, compact: compactToolbar || narrow).padding(3).glassEffect(.regular, in: Capsule())
        case .agents:
            HStack(spacing: Tokens.space[1]) {
                AgentFilterBar(model: model).padding(3).glassEffect(.regular, in: Capsule())
                ProjectMenu(model: model)
            }
        case .plans:
            PlanFilterBar(model: model).padding(3).glassEffect(.regular, in: Capsule())
        case .hosts, .messages:
            EmptyView()
        }
    }

    private var title: String { titleText(compact: compactToolbar || narrow) }

    /// The title; the compact window has no filter capsule to carry the count, so it joins the title there.
    private func titleText(compact: Bool) -> String {
        switch model.selection {
        case .inbox:
            return compact ? "Waiting for you · " + String(model.inboxTotal()) : "Waiting for you"
        case .project(let hostId, let projectId):
            // The count is of the rows below, so the title says which project filters them.
            return (projectName(hostId, projectId) ?? "Waiting for you") + " · " + String(model.inboxTotal())
        case .agents(let hostId, let projectId): return "Agents" + projectSuffix(hostId, projectId)
        case .plans: return "Plans"
        case .hosts: return "Hosts"
        case .messages: return "Messages to PM"
        }
    }

    private func projectName(_ hostId: UUID, _ projectId: String) -> String? {
        model.project(hostId, projectId).map(HostLabel.projectName)
    }

    private func projectSuffix(_ hostId: UUID, _ projectId: String) -> String {
        projectName(hostId, projectId).map { " · " + $0 } ?? ""
    }
}

/// The one filter of the inbox: All N, Proposals, Prompts, Designs, and Messages and Merges under More.
private struct KindFilter: View {
    @Bindable var model: AppModel
    /// A narrow window folds the whole filter into one menu so nothing is cut off.
    let compact: Bool

    var body: some View {
        let current = KindTab(kind: model.inboxKind)
        if compact {
            Menu {
                ForEach(KindTab.allCases, id: \.self) { tab in
                    Button { model.inboxKind = tab.kind } label: {
                        Text(verbatim: tab == .all ? "All \(model.inboxTotal())" : tab.title)
                    }
                }
            } label: {
                chip(current == .all ? "All \(model.inboxTotal())" : current.title, selected: true)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        } else {
            full(current)
        }
    }

    private func full(_ current: KindTab) -> some View {
        HStack(spacing: 2) {
            ForEach(KindTab.visible, id: \.self) { tab in
                segment(tab, selected: current == tab)
            }
            Menu {
                ForEach(KindTab.more, id: \.self) { tab in
                    Button { model.inboxKind = tab.kind } label: { Text(verbatim: tab.title) }
                }
            } label: {
                chip(KindTab.more.contains(current) ? current.title : "More", selected: KindTab.more.contains(current))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    private func segment(_ tab: KindTab, selected: Bool) -> some View {
        Button {
            model.inboxKind = tab.kind
        } label: {
            chip(tab == .all ? "All \(model.inboxTotal())" : tab.title, selected: selected)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func chip(_ text: String, selected: Bool) -> some View {
        Text(verbatim: text)
            .font(.system(size: 13, weight: selected ? .semibold : .regular))
            .foregroundStyle(selected ? Tokens.color(.accent) : Tokens.color(.label))
            .padding(.horizontal, Tokens.space[1] + 2)
            .padding(.vertical, Tokens.space[0])
            .background(selected ? Tokens.color(.fill2) : Color.clear, in: Capsule())
    }
}

/// A row of text segments in the toolbar; the selected one is filled and in the accent.
private struct Segment<T: Hashable>: Identifiable {
    let value: T
    let title: String
    var id: T { value }
}

private struct SegmentBar<T: Hashable>: View {
    let items: [Segment<T>]
    let selected: T
    let onSelect: (T) -> Void

    var body: some View {
        HStack(spacing: 2) {
            ForEach(items) { item in
                let on = item.value == selected
                Button { onSelect(item.value) } label: {
                    Text(verbatim: item.title)
                        .font(.system(size: 13, weight: on ? .semibold : .regular))
                        .foregroundStyle(on ? Tokens.color(.accent) : Tokens.color(.label))
                        .padding(.horizontal, Tokens.space[1] + 2)
                        .padding(.vertical, Tokens.space[0])
                        .background(on ? Tokens.color(.fill2) : Color.clear, in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
    }
}

/// All N, Need attention N, Paused N for the agents of the project in view.
private struct AgentFilterBar: View {
    let model: AppModel

    var body: some View {
        let rows: [AgentRow]
        if case .agents(let hostId, let projectId) = model.selection, let project = model.project(hostId, projectId) {
            rows = AgentRow.rows(project)
        } else {
            rows = []
        }
        return SegmentBar(
            items: AgentFilter.allCases.map { Segment(value: $0, title: $0.title + " " + String($0.count(rows))) },
            selected: model.agentFilter, onSelect: { model.agentFilter = $0 })
    }
}

/// Active N and Done for the plans of every project.
private struct PlanFilterBar: View {
    let model: AppModel

    var body: some View {
        let rows = model.hostModels.values.flatMap { $0.projects.values.flatMap { PlanRow.rows($0) } }
        let counts = PlanFilter.counts(rows)
        return SegmentBar(
            items: [
                Segment(value: PlanFilter.active, title: "Active \(counts.active)"),
                Segment(value: PlanFilter.drafts, title: "Drafts \(counts.drafts)"),
                Segment(value: PlanFilter.done, title: "Done"),
            ],
            selected: model.planFilter, onSelect: { model.planFilter = $0 })
    }
}

/// Agents and Plans show one project at a time; this menu picks it.
private struct ProjectMenu: View {
    let model: AppModel

    var body: some View {
        Menu {
            ForEach(model.hosts) { host in
                Section {
                    ForEach(HostLabel.projects(model.hostModels[host.id]), id: \.projectId) { project in
                        Button { open(host.id, project.projectId) } label: { Text(verbatim: HostLabel.projectName(project)) }
                    }
                } header: {
                    Text(verbatim: model.hosts.count > 1 ? HostLabel.hostName(host) : "Projects")
                }
            }
        } label: {
            Label("Project", systemImage: "folder")
        }
        .menuIndicator(.visible)
    }

    private func open(_ hostId: UUID, _ projectId: String) {
        switch model.selection {
        case .plans: model.selection = .plans(hostId: hostId, projectId: projectId)
        default: model.selection = .agents(hostId: hostId, projectId: projectId)
        }
    }
}

#Preview {
    MainWindow(model: AppModel(preview: PreviewSeed()))
        .frame(width: 1100, height: 700)
}
