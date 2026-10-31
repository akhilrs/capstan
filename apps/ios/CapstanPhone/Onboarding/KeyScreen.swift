import CapstanPhoneCore
import SwiftUI
import UIKit

/// Board 01, This iPhone's key: the public line to add to each host, Copy and Share, the Face ID switch (on by
/// default), and the way to an ed25519 key. With no enclave (the Simulator) it shows an error, never a silent
/// ed25519; a restored phone with no key says it needs a new one.
struct KeyScreen: View {
    @Bindable var model: AppModel
    let next: () -> Void
    @State private var askingEd25519 = false
    @State private var copied = false

    private var restored: Bool {
        if case .present = model.key { return false }
        return !model.hosts.isEmpty
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                if restored {
                    Text("This iPhone needs a new key").font(TypeStyle.largeTitle)
                    Text("The key that was on this iPhone did not come with the restore: a Secure Enclave key never leaves the device. Make a new one and add it to each host.")
                        .font(TypeStyle.body).foregroundStyle(Tokens.color(.label2))
                } else {
                    Text("This iPhone's key").font(TypeStyle.largeTitle)
                    intro
                }
                keyContent
                Toggle(isOn: Binding(
                    get: { model.settings.faceID },
                    set: { on in
                        var settings = model.settings
                        settings.faceID = on
                        model.updateSettings(settings)
                    })
                ) {
                    Text("Require Face ID when you open Capstan").font(TypeStyle.body)
                }
                .tint(Tokens.color(.green))
                .padding(Tokens.space[3])
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    Text("On by default; you can turn it off in Settings.")
                        .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    Button("A host that only accepts ed25519? Use an ed25519 key instead…") { askingEd25519 = true }
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.accent))
                        .frame(minHeight: 44)
                }
            }
            .padding(.horizontal, Tokens.margin)
        }
        .background(Tokens.color(.ground))
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            ActionBar {
                Button(action: next) {
                    if restored { Text("I added the new key") } else { Text("I added the key") }
                }
                .buttonStyle(.capstanPrimary)
                .disabled(!hasKey)
            }
        }
        .sheet(isPresented: $askingEd25519) {
            Ed25519Sheet(model: model)
        }
        .onAppear {
            if case .none = model.key, restored { model.makeKey(.secureEnclaveP256) }
        }
    }

    private var hasKey: Bool {
        if case .present = model.key { return true }
        return false
    }

    @ViewBuilder private var intro: some View {
        switch model.key {
        case .present(kind: .keychainEd25519, _, _):
            Text("Capstan made an ed25519 key in the Keychain. It can be included in encrypted backups and restored to another device.")
                .font(TypeStyle.body).foregroundStyle(Tokens.color(.label2))
        default:
            Text("Capstan made a key in the Secure Enclave. The private half never leaves this iPhone and cannot be exported.")
                .font(TypeStyle.body).foregroundStyle(Tokens.color(.label2))
        }
    }

    @ViewBuilder private var keyContent: some View {
        switch model.key {
        case .present(let kind, let publicLine, let fingerprint):
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                KeyFact(label: "Type", value: kind == .secureEnclaveP256 ? "ecdsa-sha2-nistp256" : "ssh-ed25519")
                KeyFact(label: "Stored", value: kind == .secureEnclaveP256 ? "Secure Enclave" : "Keychain, can be backed up")
                KeyFact(label: "Fingerprint", value: fingerprint)
            }
            .padding(Tokens.space[3])
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text("Public key · add it to each host").font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                Text(verbatim: publicLine)
                    .font(TypeStyle.monoSmall)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(Tokens.space[3])
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            HStack(spacing: Tokens.space[2]) {
                Button {
                    UIPasteboard.general.string = publicLine
                    copied = true
                    Haptics.play(.success)
                } label: {
                    if copied {
                        Label("Copied", systemImage: "checkmark")
                    } else {
                        Label("Copy", systemImage: "doc.on.doc")
                    }
                }
                .buttonStyle(.capstanQuiet)
                ShareLink(item: publicLine) {
                    Label("Share…", systemImage: "square.and.arrow.up")
                }
                .buttonStyle(.capstanQuiet)
            }
            Text("On each host, append this line to ~/.ssh/authorized_keys for the user that runs Capstan.")
                .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
        case .unavailable(let reason):
            Banner(tone: .red, text: reason)
            Text("Without a Secure Enclave, Capstan can only use an ed25519 key from the Keychain. Choose it below if you accept that it can be backed up.")
                .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
        case .none:
            HStack(spacing: Tokens.space[1]) {
                ProgressView()
                Text("Making the key…").font(TypeStyle.body)
            }
        }
    }
}

private struct KeyFact: View {
    let label: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(verbatim: label).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2)).frame(width: 96, alignment: .leading)
            Text(verbatim: value).font(TypeStyle.monoSmall).textSelection(.enabled)
        }
        .accessibilityElement(children: .combine)
    }
}

/// The ed25519 fallback: the backup warning, with keeping the Secure Enclave key as the focused default.
struct Ed25519Sheet: View {
    @Bindable var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @FocusState private var keepFocused: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text("Use an ed25519 key?").font(TypeStyle.title2)
                Text("Some hosts accept only ssh-ed25519. The Secure Enclave cannot hold that type, so Capstan would keep this key in the Keychain instead.")
                    .font(TypeStyle.body)
                Banner(
                    tone: .yellow, text: "This key can leave the iPhone",
                    detail: nil)
                Text("A Keychain key can be included in encrypted backups and restored to another device. The Secure Enclave key cannot. Keep the Secure Enclave key for hosts that accept it.")
                    .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    KeyFact(label: "Type", value: "ssh-ed25519")
                    KeyFact(label: "Stored", value: "Keychain, can be backed up")
                }
                VStack(spacing: Tokens.space[2]) {
                    Button("Keep the Secure Enclave key") { dismiss() }
                        .buttonStyle(.capstanQuiet)
                        .overlay(Capsule().strokeBorder(Tokens.color(.accent), lineWidth: keepFocused ? 2 : 0))
                        .focused($keepFocused)
                        .keyboardShortcut(.cancelAction)
                    Button("Make an ed25519 key") {
                        model.makeKey(.keychainEd25519)
                        dismiss()
                    }
                    .buttonStyle(.capstanPrimary)
                }
            }
            .padding(Tokens.margin)
        }
        .presentationDetents([.large])
        .presentationBackground(Tokens.color(.cell))
        .onAppear { keepFocused = true }
    }
}

#Preview("Key") {
    NavigationStack { KeyScreen(model: AppModel(preview: PreviewData.standard), next: {}) }
}

#Preview("Key, no enclave") {
    var seed = PreviewData.empty
    seed.key = .unavailable("This device has no Secure Enclave.")
    return NavigationStack { KeyScreen(model: AppModel(preview: seed), next: {}) }
}

#Preview("Key, restored phone") {
    var seed = PreviewData.standard
    seed.key = .none
    return NavigationStack { KeyScreen(model: AppModel(preview: seed), next: {}) }.preferredColorScheme(.dark)
}

#Preview("ed25519 sheet") {
    Color.clear.sheet(isPresented: .constant(true)) { Ed25519Sheet(model: AppModel(preview: PreviewData.standard)) }
}
