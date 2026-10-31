import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI
import UIKit

/// Settings (board 14), pushed from the Hosts toolbar: hosts, this iPhone's key, Face ID (on by default), the poll
/// interval with its 1000 ms minimum shown inline, the heartbeat pulse, notifications and sounds, the Live Activity,
/// and the version. Capstan has no server, so there is nothing to switch on for remote alerts.
struct SettingsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            Section {
                Button { model.navigation.show(.hosts, []) } label: {
                    LabeledContent {
                        Text(verbatim: String(model.hosts.count))
                    } label: {
                        Text("Hosts").foregroundStyle(Tokens.color(.label))
                    }
                }
                NavigationLink {
                    KeySettingsScreen()
                } label: {
                    LabeledContent {
                        Text(verbatim: keyKind)
                    } label: {
                        Text("This iPhone's key")
                    }
                }
                Toggle(isOn: binding(\.faceID)) { Text("Require Face ID") }
            } header: {
                Text("Connection")
            } footer: {
                Text("Face ID is asked when Capstan opens and after a minute away, before any host is reached.")
            }
            Section {
                PollIntervalRow()
                Toggle(isOn: binding(\.heartbeatPulse)) { Text("Heartbeat pulse") }
            } header: {
                Text("Following")
            } footer: {
                Text("The poll interval applies to every host the next time it connects. The pulse stops under Reduce Motion.")
            }
            Section {
                NavigationLink {
                    NotificationsSettingsScreen()
                } label: {
                    LabeledContent {
                        Text(verbatim: model.settings.notifyKinds.isEmpty ? "Off" : "On")
                    } label: {
                        Text("Notifications and sounds")
                    }
                }
                Toggle(isOn: binding(\.liveActivityAllowed)) { Text("Live Activity when items wait") }
            } header: {
                Text("Alerts")
            } footer: {
                Text("Capstan has no server, so alerts come only while the app follows your hosts. Light and dark follow the system.")
            }
            Section {
                LabeledContent {
                    Text(verbatim: version)
                } label: {
                    Text("Version")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Settings")
        .navigationBarTitleDisplayMode(.large)
    }

    private var keyKind: String {
        switch model.key {
        case .present(let kind, _, _): return kind == .secureEnclaveP256 ? "Secure Enclave" : "ed25519"
        case .none: return "None"
        case .unavailable: return "Unavailable"
        }
    }

    private var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return short + " (" + build + ")"
    }

    private func binding(_ path: WritableKeyPath<PhoneSettings, Bool>) -> Binding<Bool> {
        Binding(
            get: { model.settings[keyPath: path] },
            set: { on in
                var settings = model.settings
                settings[keyPath: path] = on
                model.updateSettings(settings)
            })
    }
}

/// The poll interval: the value in ms, a stepper in 500 ms steps kept within the limits, and the inline error when
/// a saved value is below the 1000 ms minimum (the stepper's first step clamps it).
struct PollIntervalRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pollMs = model.settings.pollMs
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            LabeledContent {
                Text(verbatim: String(pollMs) + " ms").font(TypeStyle.body.monospacedDigit())
            } label: {
                Text("Poll interval")
            }
            HStack(alignment: .center) {
                if let error = SettingsValidation.pollError(pollMs) {
                    Label { Text(verbatim: error) } icon: { Image(systemName: "exclamationmark.triangle") }
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.red))
                }
                Spacer()
                Stepper {
                    Text("Poll interval")
                } onIncrement: {
                    set(SettingsValidation.step(pollMs, up: true))
                } onDecrement: {
                    set(SettingsValidation.step(pollMs, up: false))
                }
                .labelsHidden()
            }
        }
    }

    private func set(_ value: Int) {
        var settings = model.settings
        settings.pollMs = value
        model.updateSettings(settings)
        Haptics.play(.selection)
    }
}

/// This iPhone's key: its kind, the public line to put in `authorized_keys` (Copy and Share), the fingerprint, and
/// Replace…, which confirms first because every host refuses the new key until its line is added there.
struct KeySettingsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var confirmReplace = false
    @State private var copied = false

    var body: some View {
        List {
            switch model.key {
            case .present(let kind, let publicLine, let fingerprint):
                Section {
                    LabeledContent {
                        Text(verbatim: kind == .secureEnclaveP256 ? "Secure Enclave P-256" : "ed25519, in the Keychain")
                    } label: {
                        Text("Kind")
                    }
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        Text("Public line").font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                        Text(verbatim: publicLine).font(TypeStyle.monoSmall).textSelection(.enabled)
                    }
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        Text("Fingerprint").font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                        Text(verbatim: fingerprint).font(TypeStyle.monoSmall).textSelection(.enabled)
                    }
                } footer: {
                    Text("Add the public line to ~/.ssh/authorized_keys on each host, on a line of its own.")
                }
                Section {
                    Button {
                        UIPasteboard.general.string = publicLine
                        copied = true
                        Haptics.play(.success)
                    } label: {
                        if copied {
                            Label("Copied", systemImage: "checkmark")
                        } else {
                            Label("Copy the public line", systemImage: "doc.on.doc")
                        }
                    }
                    ShareLink(item: publicLine) { Label("Share the public line", systemImage: "square.and.arrow.up") }
                }
                Section {
                    Button(role: .destructive) { confirmReplace = true } label: { Text("Replace the key…") }
                } footer: {
                    Text("A new key is made on this iPhone. Every host refuses it until you add its public line there.")
                }
                .sheet(isPresented: $confirmReplace) {
                    ConfirmSheet(
                        title: "Replace this iPhone's key?", detail: nil, noteLimit: nil, destructiveLabel: "Replace the key",
                        footnote: "Every host refuses the new key until you add its public line to authorized_keys there. The old line can then be deleted.",
                        onConfirm: { _ in model.makeKey(kind) })
                }
            case .none:
                Section {
                    Text("This iPhone needs a new key.")
                    Button("Make a key") { model.makeKey(.secureEnclaveP256) }
                }
            case .unavailable(let reason):
                Section {
                    Text(verbatim: reason)
                    Button("Use an ed25519 key") { model.makeKey(.keychainEd25519) }
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("This iPhone's key")
        .navigationBarTitleDisplayMode(.inline)
    }
}

#Preview("Settings") {
    NavigationStack { SettingsScreen() }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Settings, dark") {
    NavigationStack { SettingsScreen() }
        .environment(AppModel(preview: PreviewData.standard))
        .preferredColorScheme(.dark)
}

#Preview("Key") {
    NavigationStack { KeySettingsScreen() }
        .environment(AppModel(preview: PreviewData.standard))
}
