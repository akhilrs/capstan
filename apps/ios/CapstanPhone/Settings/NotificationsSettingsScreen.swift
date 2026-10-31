import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Settings → Notifications (board 14): the kinds that make a notification, the default sound and a sound per kind,
/// and Time Sensitive per kind. A notification says the kind and the project only; Capstan has no server, so they come
/// while the app follows the hosts.
struct NotificationsSettingsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let catalog = PhoneSounds.catalog()
        List {
            Section {
                ForEach(WaitingKind.settingsKinds, id: \.wireName) { kind in
                    Toggle(isOn: binding(kind, in: \.notifyKinds)) { Text(verbatim: kind.settingsName) }
                }
            } header: {
                Text("Notify me about")
            }
            Section {
                NavigationLink {
                    SoundPickerScreen(kind: nil)
                } label: {
                    LabeledContent {
                        Text(verbatim: catalog.title(of: model.settings.sounds.defaultSound))
                    } label: {
                        Text("Default sound")
                    }
                }
                ForEach(WaitingKind.settingsKinds, id: \.wireName) { kind in
                    NavigationLink {
                        SoundPickerScreen(kind: kind)
                    } label: {
                        LabeledContent {
                            Text(verbatim: model.settings.sounds.perKind[kind.wireName].map { catalog.title(of: $0) } ?? "Use default")
                        } label: {
                            Text(verbatim: kind.settingsName)
                        }
                    }
                }
            } header: {
                Text("Sounds")
            }
            Section {
                ForEach(WaitingKind.settingsKinds, id: \.wireName) { kind in
                    Toggle(isOn: binding(kind, in: \.timeSensitiveKinds)) { Text(verbatim: kind.settingsName) }
                }
            } header: {
                Text("Time Sensitive")
            } footer: {
                Text("A Time Sensitive notification can break through a Focus when you allow it in iOS Settings. Notifications say the kind and the project, never the text.")
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Notifications")
        .navigationBarTitleDisplayMode(.large)
    }

    private func binding(_ kind: WaitingKind, in path: WritableKeyPath<PhoneSettings, Set<WaitingKind>>) -> Binding<Bool> {
        Binding(
            get: { model.settings[keyPath: path].contains(kind) },
            set: { on in
                var settings = model.settings
                if on { settings[keyPath: path].insert(kind) } else { settings[keyPath: path].remove(kind) }
                model.updateSettings(settings)
                Haptics.play(.selection)
            })
    }
}

#Preview("Notifications") {
    NavigationStack { NotificationsSettingsScreen() }
        .environment(AppModel(preview: PreviewData.standard))
}
