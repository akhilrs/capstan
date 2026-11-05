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
    /// What the Agents and Plans lists show and search; kept while the person moves between all projects and one.
    @State private var agentShow = AgentShow.all
    @State private var planShow = PlanShow.active
    @State private var listQuery = ""
    @State private var agentPick: AgentPick?
    @FocusState private var searchFocused: Bool

    private static let foldWidth: CGFloat = 900
    /// The widths the toolbar needs beside the title: the whole filter, the search field and the gaps.
    private static let filterWidth: CGFloat = 440
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

    private var removalName: String {
        guard let key = model.projectToRemove, let project = model.project(key.hostId, key.projectId) else { return "this project" }
        return HostLabel.projectName(project)
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
        .alert(
            ProjectRemovalPrompt.title(name: removalName),
            isPresented: Binding(get: { model.projectToRemove != nil }, set: { if !$0 { model.projectToRemove = nil } }),
            presenting: model.projectToRemove
        ) { key in
            Button(ProjectRemovalPrompt.confirmTitle, role: .destructive) { // static-text
                Task { await model.removeProject(hostId: key.hostId, projectId: key.projectId) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text(ProjectRemovalPrompt.message) // static-text
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
                        .frame(height: ToolbarMetrics.capsuleHeight)
                        .glassEffect(.regular, in: Capsule())
                    filterCapsule
                }
            }
            .sharedBackgroundVisibility(.hidden)
            ToolbarItem(placement: .primaryAction) {
                switch model.selection {
                case .hosts:
                    Button {
                        if model.presented == nil { model.presented = .addHost(editing: nil) }
                    } label: {
                        Label { Text("Add host…") } icon: { Image(systemName: "square.and.pencil") }
                    }
                case .plans, .allPlans:
                    // A new plan starts as a message to the PM.
                    Button {
                        model.selection = .messages
                    } label: {
                        Label { Text("New plan") } icon: { Image(systemName: "square.and.pencil") }
                    }
                default:
                    EmptyView()
                }
            }
            if showsListSearch {
                // The field draws its own glass capsule; the system's shared background would wrap a second one around it.
                ToolbarItem(placement: .primaryAction) {
                    ListSearchField(
                        text: $listQuery, prompt: listSearchPrompt, focus: $searchFocused)
                }
                .sharedBackgroundVisibility(.hidden)
            }
            ToolbarItem(placement: .primaryAction) {
                MessagePMButton(model: model)
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
        .onChange(of: model.selection) { _, selection in
            agentPick = nil
            switch selection {
            case .agents, .allAgents, .plans, .allPlans: break
            default: listQuery = ""
            }
        }
        .background {
            // Command-F reaches the search field of Agents and Plans.
            Button("") { searchFocused = true }
                .keyboardShortcut("f", modifiers: .command)
                .opacity(0)
                .disabled(!showsListSearch)
                .accessibilityHidden(true)
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
        case .allAgents:
            AllAgentsView(
                model: model, show: agentShow, query: listQuery, pick: $agentPick,
                onShow: { agentShow = $0 }, onClearSearch: { listQuery = "" })
        case .agents(let hostId, let projectId):
            AgentsScreen(
                model: model, hostId: hostId, projectId: projectId, show: agentShow, query: listQuery,
                onShow: { agentShow = $0 }, onClearSearch: { listQuery = "" })
        case .allPlans:
            PlansScreen(
                model: model, scope: nil, show: planShow, query: listQuery,
                onShow: { planShow = $0 }, onClearSearch: { listQuery = "" })
        case .plans(let hostId, let projectId):
            PlansScreen(
                model: model, scope: ProjectKey(hostId: hostId, projectId: projectId), show: planShow, query: listQuery,
                onShow: { planShow = $0 }, onClearSearch: { listQuery = "" })
        case .hosts:
            HostsScreen(model: model)
        case .messages:
            MessagesScreen(model: model)
        }
    }

    @ViewBuilder
    private var inspector: some View {
        switch model.selection {
        case .inbox, .project:
            InspectorScreen(model: model, row: model.selectedRow)
        case .hosts:
            HostsInspector(model: model)
        case .allAgents:
            if let pick = agentPick {
                if pick.agentId != nil {
                    AgentInspector(model: model, hostId: pick.hostId, projectId: pick.projectId, allProjects: true)
                } else {
                    ProjectGroupInspector(model: model, hostId: pick.hostId, projectId: pick.projectId)
                }
            } else {
                Text("Select an agent to see what it is doing")
                    .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        case .agents(let hostId, let projectId):
            AgentInspector(model: model, hostId: hostId, projectId: projectId)
        case .plans, .allPlans:
            PlanInspector(model: model)
        case .messages:
            EmptyView()
        }
    }

    private var hasInspector: Bool {
        switch model.selection {
        case .inbox, .project, .hosts, .agents, .plans, .allAgents, .allPlans: return true
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
        case .agents, .allAgents:
            HStack(spacing: Tokens.space[1]) {
                ProjectMenu(model: model)
                AgentFilterBar(model: model, show: $agentShow, query: listQuery).padding(.horizontal, 3)
                    .frame(height: ToolbarMetrics.capsuleHeight)
                    .glassEffect(.regular, in: Capsule())
            }
        case .plans, .allPlans:
            HStack(spacing: Tokens.space[1]) {
                ProjectMenu(model: model)
                PlanFilterBar(model: model, show: $planShow, query: listQuery).padding(.horizontal, 3)
                    .frame(height: ToolbarMetrics.capsuleHeight)
                    .glassEffect(.regular, in: Capsule())
            }
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
        case .allAgents: return "Agents"
        case .agents: return "Agents"
        case .allPlans, .plans: return "Plans"
        case .hosts: return "Hosts"
        case .messages: return "Messages to PM"
        }
    }

    private var showsListSearch: Bool {
        switch model.selection {
        case .agents, .allAgents, .plans, .allPlans: return true
        default: return false
        }
    }

    private var listSearchPrompt: String {
        switch model.selection {
        case .plans, .allPlans: return "Plan titles and ids"
        default: return "Agents, roles, projects"
        }
    }

    private func projectName(_ hostId: UUID, _ projectId: String) -> String? {
        model.project(hostId, projectId).map(HostLabel.projectName)
    }
}

/// The one filter of the inbox: All N, Proposals, Prompts, Designs, Messages and Merges.
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

/// One height for every capsule in the toolbar, so the title, project menu, filter and search line up.
enum ToolbarMetrics {
    static let capsuleHeight: CGFloat = 30
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

/// All, Attention, Working and Paused with their counts for the agents in view; the counts follow the search.
private struct AgentFilterBar: View {
    let model: AppModel
    @Binding var show: AgentShow
    let query: String

    var body: some View {
        let scope = currentScope(model)
        let counts = AgentShow.counts(
            ListData.agentGroups(model, scope: scope), needle: query.trimmingCharacters(in: .whitespaces))
        return SegmentBar(
            items: AgentShow.allCases.map { Segment(value: $0, title: $0.title + " " + String(counts[$0] ?? 0)) },
            selected: show, onSelect: { show = $0 })
    }
}

/// Active, In review and Done with their counts for the plans in view; the counts follow the search.
private struct PlanFilterBar: View {
    let model: AppModel
    @Binding var show: PlanShow
    let query: String

    var body: some View {
        let scope = currentScope(model)
        let groups = ListData.planGroups(model, scope: scope, pinned: [])
        let counts = PlanShow.counts(ListData.planItems(groups), needle: query.trimmingCharacters(in: .whitespaces))
        return SegmentBar(
            items: PlanShow.allCases.map { Segment(value: $0, title: $0.title + " " + String(counts[$0] ?? 0)) },
            selected: show, onSelect: { show = $0 })
    }
}

/// The project the Agents or Plans list is narrowed to, or nil for all projects.
@MainActor
private func currentScope(_ model: AppModel) -> ProjectKey? {
    switch model.selection {
    case .agents(let hostId, let projectId), .plans(let hostId, let projectId):
        return ProjectKey(hostId: hostId, projectId: projectId)
    default: return nil
    }
}

/// Agents and Plans show every project or one; this menu picks which, and keeps the person on the same list.
private struct ProjectMenu: View {
    let model: AppModel
    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""

    var body: some View {
        let scope = currentScope(model)
        let pinned = ProjectPins.list(pinnedRaw)
        Menu {
            Button { open(nil) } label: {
                Text(verbatim: (scope == nil ? "✓ " : "") + "All projects")
            }
            ForEach(model.hosts) { host in
                Section {
                    ForEach(pinnedFirst(host, pinned), id: \.projectId) { project in
                        let current = scope == ProjectKey(hostId: host.id, projectId: project.projectId)
                        Button { open(ProjectKey(hostId: host.id, projectId: project.projectId)) } label: {
                            Text(verbatim: (current ? "✓ " : "") + HostLabel.projectName(project))
                        }
                    }
                } header: {
                    Text(verbatim: model.hosts.count > 1 ? HostLabel.hostName(host) : "Projects")
                }
            }
        } label: {
            // Drawn like the title and filter capsules beside it: same height, label colour, its own chevron.
            HStack(spacing: Tokens.space[0] + 1) {
                Text(verbatim: label(scope))
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(Tokens.color(.label2))
                    .accessibilityHidden(true)
            }
            .foregroundStyle(Tokens.color(.label))
            .padding(.horizontal, Tokens.space[2])
            .frame(height: ToolbarMetrics.capsuleHeight)
            .contentShape(Capsule())
            .glassEffect(.regular.interactive(), in: Capsule())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel("Project")
    }

    /// The host's projects with the pinned ones first.
    private func pinnedFirst(_ host: HostConfig, _ pinned: [String]) -> [ProjectModel] {
        let all = HostLabel.projects(model.hostModels[host.id])
        let isPinned = { (project: ProjectModel) in pinned.contains(host.id.uuidString + "/" + project.projectId) }
        return all.filter(isPinned) + all.filter { !isPinned($0) }
    }

    private func label(_ scope: ProjectKey?) -> String {
        guard let scope, let project = model.project(scope.hostId, scope.projectId) else { return "All projects" }
        return HostLabel.projectName(project)
    }

    private func open(_ key: ProjectKey?) {
        let plans: Bool
        switch model.selection {
        case .plans, .allPlans: plans = true
        default: plans = false
        }
        switch (plans, key) {
        case (true, let key?): model.selection = .plans(hostId: key.hostId, projectId: key.projectId)
        case (true, nil): model.selection = .allPlans
        case (false, let key?): model.selection = .agents(hostId: key.hostId, projectId: key.projectId)
        case (false, nil): model.selection = .allAgents
        }
    }
}

#Preview {
    MainWindow(model: AppModel(preview: PreviewSeed()))
        .frame(width: 1100, height: 700)
}
