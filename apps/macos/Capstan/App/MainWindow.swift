import CapstanAppCore
import CapstanKit
import SwiftUI

/// Sidebar, content, inspector. Under `foldWidth` the sidebar folds and the inspector moves into a sheet.
struct MainWindow: View {
    @Bindable var model: AppModel
    @State private var columnVisibility = NavigationSplitViewVisibility.all
    @State private var narrow = false
    @State private var inspectorShown = true
    @State private var inspectorSheet = false

    private static let foldWidth: CGFloat = 900
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
            ToolbarItem(placement: .navigation) {
                Text(verbatim: title)
                    .font(.system(size: 15, weight: .semibold))
                    .padding(.horizontal, Tokens.space[1])
            }
            ToolbarItem(placement: .principal) {
                if showsKindFilter {
                    KindFilter(model: model)
                } else if showsProjectMenu {
                    ProjectMenu(model: model)
                }
            }
        }
        .tint(Tokens.color(.accent))
        .frame(minWidth: 640, minHeight: 480)
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
        case .plans(let hostId, let projectId):
            PlansScreen(model: model, hostId: hostId, projectId: projectId)
        case .hosts:
            HostsScreen(model: model)
        }
    }

    @ViewBuilder
    private var inspector: some View {
        switch model.selection {
        case .inbox, .project:
            InspectorScreen(model: model, row: model.selectedRow)
        case .hosts:
            HostsInspector(model: model)
        case .agents, .plans:
            EmptyView()
        }
    }

    private var hasInspector: Bool {
        switch model.selection {
        case .inbox, .project, .hosts: return true
        case .agents, .plans: return false
        }
    }

    /// Wide windows show the inspector as a column; narrow ones never do.
    private var inspectorBinding: Binding<Bool> {
        Binding(
            get: { !narrow && hasInspector && inspectorShown },
            set: { inspectorShown = $0 })
    }

    private var title: String {
        switch model.selection {
        case .inbox: return model.inboxKind == .message ? "Messages to PM" : "Waiting for you"
        case .project(let hostId, let projectId): return projectName(hostId, projectId) ?? "Waiting for you"
        case .agents(let hostId, let projectId): return "Agents" + projectSuffix(hostId, projectId)
        case .plans(let hostId, let projectId): return "Plans" + projectSuffix(hostId, projectId)
        case .hosts: return "Hosts"
        }
    }

    private func projectName(_ hostId: UUID, _ projectId: String) -> String? {
        model.project(hostId, projectId).map(HostLabel.projectName)
    }

    private func projectSuffix(_ hostId: UUID, _ projectId: String) -> String {
        projectName(hostId, projectId).map { " · " + $0 } ?? ""
    }

    private var showsKindFilter: Bool {
        switch model.selection {
        case .inbox, .project: return true
        default: return false
        }
    }

    private var showsProjectMenu: Bool {
        switch model.selection {
        case .agents, .plans: return true
        default: return false
        }
    }
}

/// The one filter of the inbox: All N, Proposals, Prompts, Designs, and Messages and Merges under More.
private struct KindFilter: View {
    @Bindable var model: AppModel

    var body: some View {
        let current = KindTab(kind: model.inboxKind)
        HStack(spacing: 2) {
            ForEach(KindTab.visible, id: \.self) { tab in
                segment(tab, selected: current == tab)
            }
            Menu {
                ForEach(KindTab.more, id: \.self) { tab in
                    Button(tab.title) { model.inboxKind = tab.kind }
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

/// Agents and Plans show one project at a time; this menu picks it.
private struct ProjectMenu: View {
    let model: AppModel

    var body: some View {
        Menu {
            ForEach(model.hosts) { host in
                Section(model.hosts.count > 1 ? HostLabel.hostName(host) : "Projects") {
                    ForEach(HostLabel.projects(model.hostModels[host.id]), id: \.projectId) { project in
                        Button(HostLabel.projectName(project)) { open(host.id, project.projectId) }
                    }
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
