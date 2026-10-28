import CapstanAppCore
import CapstanKit
import SwiftUI

enum SettingsTab: Hashable {
    case general, hosts, notifications
}

struct SettingsScreen: View {
    let model: AppModel
    @State private var tab: SettingsTab

    init(model: AppModel, tab: SettingsTab = .general) {
        self.model = model
        _tab = State(initialValue: tab)
    }

    var body: some View {
        TabView(selection: $tab) {
            GeneralSettings(model: model)
                .tabItem { Label("General", systemImage: "gearshape") }
                .tag(SettingsTab.general)
            HostsSettings(model: model)
                .tabItem { Label("Hosts", systemImage: "server.rack") }
                .tag(SettingsTab.hosts)
            NotificationSettings()
                .tabItem { Label("Notifications", systemImage: "bell") }
                .tag(SettingsTab.notifications)
        }
        .frame(width: 560, height: 480)
    }
}

private struct GeneralSettings: View {
    let model: AppModel
    @State private var draft: String
    @FocusState private var focused: Bool

    init(model: AppModel) {
        self.model = model
        _draft = State(initialValue: model.settings.clientLabel)
    }

    var body: some View {
        Form {
            Section {
                TextField("Client label", text: $draft)
                    .focused($focused)
                    .onSubmit(commit)
                    .onChange(of: focused) { _, now in if !now { commit() } }
                Text("Sent with every decision, so the host can tell this Mac from other clients.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            Section("Notify me about") {
                ForEach(Self.kinds, id: \.wireName) { kind in
                    Toggle(isOn: kindBinding(kind)) { Text(verbatim: kind.groupTitle) }
                }
            }
            Section {
                Toggle("Heartbeat pulse", isOn: binding(\.heartbeatPulse))
                Toggle("Allow during Focus", isOn: binding(\.allowDuringFocus))
                Text(
                    "Needs the Time Sensitive Notifications entitlement, which you add to the app by hand. "
                        + "Without it macOS delivers the notification at the normal level and Focus keeps it quiet.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .formStyle(.grouped)
        .onDisappear(perform: commit)
    }

    private static let kinds: [WaitingKind] = [.op, .prompt, .design, .message, .integration]

    private func commit() {
        let trimmed = String(draft.trimmingCharacters(in: .whitespacesAndNewlines).prefix(32))
        if trimmed.isEmpty {
            draft = model.settings.clientLabel
        } else if trimmed != model.settings.clientLabel {
            var next = model.settings
            next.clientLabel = trimmed
            model.updateSettings(next)
        }
    }

    private func binding<T>(_ keyPath: WritableKeyPath<AppSettings, T>) -> Binding<T> {
        Binding(
            get: { model.settings[keyPath: keyPath] },
            set: { value in
                var next = model.settings
                next[keyPath: keyPath] = value
                model.updateSettings(next)
            })
    }

    private func kindBinding(_ kind: WaitingKind) -> Binding<Bool> {
        Binding(
            get: { model.settings.notifyKinds.contains(kind) },
            set: { on in
                var next = model.settings
                if on { next.notifyKinds.insert(kind) } else { next.notifyKinds.remove(kind) }
                model.updateSettings(next)
            })
    }
}

private struct HostsSettings: View {
    @Bindable var model: AppModel
    @Environment(\.controlActiveState) private var activeState

    init(model: AppModel) {
        self.model = model
    }

    /// The add-host sheet is shown only by the key window, so it does not open in the main window as well.
    private var sheetItem: Binding<AppSheet?> {
        Binding(
            get: { activeState == .key ? model.presented : nil },
            set: { if activeState == .key { model.presented = $0 } })
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                HostsListView(model: model)
                ForEach(model.hosts) { host in
                    if let error = HostLimits.pollError(host) {
                        Text(verbatim: "\(HostLabel.hostName(host)): poll interval. \(error)")
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
                    }
                }
                Text(
                    "Signing in uses your SSH agent. The key file path is the only thing kept; "
                        + "no key, passphrase or password is stored.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            .padding(Tokens.space[3])
        }
        .sheet(item: sheetItem) { sheet in
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
    }
}

private struct NotificationSettings: View {
    @State private var preferences = SoundPreferences.load(from: UserDefaultsSoundStore())
    @State private var catalog: SoundCatalog
    @State private var status: AuthStatus?
    @State private var player = SoundPlayer()
    private let store = UserDefaultsSoundStore()
    private let source: @Sendable () async -> AuthStatus

    init(
        catalog: SoundCatalog = SoundLibrary.catalog(),
        status: @escaping @Sendable () async -> AuthStatus = { await NotificationCenterAdapter().authorizationStatus() }
    ) {
        _catalog = State(initialValue: catalog)
        source = status
    }

    private static let kinds: [WaitingKind] = [.op, .prompt, .design, .message, .integration]

    var body: some View {
        Form {
            if status == .denied {
                Section {
                    Text("Notifications for Capstan are off in System Settings, so no sound or banner will show.")
                        .font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
                }
            }
            Section("Default sound") {
                SoundPicker(
                    title: "Default", selection: defaultBinding, catalog: catalog, player: player)
            }
            Section("Sound per kind") {
                ForEach(Self.kinds, id: \.wireName) { kind in
                    SoundPicker(
                        title: kind.groupTitle, selection: kindBinding(kind), catalog: catalog,
                        inherited: preferences.defaultSound, player: player)
                }
            }
            Section {
                Text(
                    "Sounds come from the system alert sounds and your own folder. "
                        + "A notification follows the alert volume; the preview plays at the output volume.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .formStyle(.grouped)
        .onAppear { catalog = SoundLibrary.catalog() }
        .task { status = await source() }
    }

    private var defaultBinding: Binding<SoundChoice?> {
        Binding(
            get: { preferences.defaultSound },
            set: { value in
                guard let value else { return }
                preferences.defaultSound = value
                preferences.save(to: store)
            })
    }

    private func kindBinding(_ kind: WaitingKind) -> Binding<SoundChoice?> {
        Binding(
            get: { preferences.perKind[kind.wireName] },
            set: { value in
                preferences.perKind[kind.wireName] = value
                preferences.save(to: store)
            })
    }
}

#Preview("Settings: general") {
    SettingsScreen(model: AppModel(preview: PreviewData.standard))
}

#Preview("Settings: hosts with a poll error") {
    var seed = PreviewData.standard
    if !seed.hosts.isEmpty { seed.hosts[0].pollMs = 10 }
    return SettingsScreen(model: AppModel(preview: seed), tab: .hosts)
}

#Preview("Settings: notifications") {
    SettingsScreen(model: AppModel(preview: PreviewData.standard), tab: .notifications)
}
