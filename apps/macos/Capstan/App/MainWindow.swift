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
    private static let kindFilter: [WaitingKind?] = [nil, .op, .prompt, .design, .message, .integration]

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
            ToolbarItem(placement: .principal) {
                StatusCapsule(model: model, hostId: currentHostId)
            }
            ToolbarItem(placement: .automatic) {
                kindPicker
            }
        }
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

    private var currentHostId: UUID? {
        switch model.selection {
        case .project(let hostId, _), .agents(let hostId, _), .plans(let hostId, _): return hostId
        case .hosts: return model.hostsFocus?.hostId
        case .inbox: return nil
        }
    }

    private var kindIndex: Binding<Int> {
        Binding(
            get: { Self.kindFilter.firstIndex(where: { $0 == model.inboxKind }) ?? 0 },
            set: { model.inboxKind = Self.kindFilter[$0] })
    }

    private var kindPicker: some View {
        Picker("Kind", selection: kindIndex) {
            Text("All").tag(0)
            Text("Ops").tag(1)
            Text("Prompts").tag(2)
            Text("Designs").tag(3)
            Text("Messages").tag(4)
            Text("Integrations").tag(5)
        }
        .pickerStyle(.segmented)
        .disabled(!showsKindFilter)
    }

    private var showsKindFilter: Bool {
        switch model.selection {
        case .inbox, .project: return true
        default: return false
        }
    }
}

#Preview {
    MainWindow(model: AppModel(preview: PreviewSeed()))
        .frame(width: 1100, height: 700)
}
