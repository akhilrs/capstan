import CapstanPhoneCore
import SwiftUI

/// Board 01, Trust the host key: the SHA256 fingerprint the host showed and the command to compare it on the host.
/// Cancel is the default and has the focus.
struct TrustSheet: View {
    let pin: HostKeyPin
    let hostDisplay: String
    let onTrust: () -> Void
    let onCancel: () -> Void

    @Environment(\.dismiss) private var dismiss
    @FocusState private var cancelFocused: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text("First connection").font(TypeStyle.title2)
                Text(verbatim: "This iPhone has not seen " + hostDisplay
                    + " before. Check that the fingerprint matches the host before you trust it.")
                    .font(TypeStyle.body)
                FingerprintBlock(label: TrustSheet.keyName(pin.algorithm) + " host key · from the host", fingerprint: pin.fingerprint)
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text("On the host:").font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: TrustSheet.compareCommand(pin.algorithm))
                        .font(TypeStyle.monoSmall)
                        .foregroundStyle(Tokens.color(.label2))
                        .textSelection(.enabled)
                }
                HStack(spacing: Tokens.space[2]) {
                    Button("Cancel") {
                        onCancel()
                        dismiss()
                    }
                    .buttonStyle(.capstanQuiet)
                    .overlay(Capsule().strokeBorder(Tokens.color(.accent), lineWidth: cancelFocused ? 2 : 0))
                    .focused($cancelFocused)
                    .keyboardShortcut(.cancelAction)
                    Button("Trust and connect") {
                        onTrust()
                        dismiss()
                    }
                    .buttonStyle(.capstanPrimary)
                }
            }
            .padding(Tokens.margin)
        }
        .presentationDetents([.medium, .large])
        .presentationBackground(Tokens.color(.cell))
        .interactiveDismissDisabled()
        .onAppear { cancelFocused = true }
    }

    /// The name the design uses: ED25519, ECDSA, RSA.
    static func keyName(_ algorithm: String) -> String {
        if algorithm == "ssh-ed25519" { return "ED25519" }
        if algorithm.hasPrefix("ecdsa-") { return "ECDSA" }
        if algorithm.contains("rsa") { return "RSA" }
        return algorithm.uppercased()
    }

    /// `ssh-keygen -lf` on the host's public key file of that type.
    static func compareCommand(_ algorithm: String) -> String {
        let type: String
        if algorithm == "ssh-ed25519" {
            type = "ed25519"
        } else if algorithm.hasPrefix("ecdsa-") {
            type = "ecdsa"
        } else {
            type = "rsa"
        }
        return "ssh-keygen -lf /etc/ssh/ssh_host_" + type + "_key.pub"
    }
}

/// A fingerprint in a flat mono block with its label.
struct FingerprintBlock: View {
    let label: String
    let fingerprint: String

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: label).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: fingerprint)
                .font(TypeStyle.mono)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[3])
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .combine)
    }
}

/// Board 01, Host key changed: blocked. No "trust anyway": the way back is to remove the host and add it again after
/// comparing fingerprints (plan section 5.6).
struct HostKeyChangedView: View {
    @Bindable var model: AppModel
    let hostId: UUID
    @State private var confirmingRemove = false
    @Environment(\.dismiss) private var dismiss

    private var display: String {
        model.phoneHost(hostId)?.display ?? "This host"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Banner(tone: .red, text: "The host key changed")
                Text(verbatim: display + " now shows a different key than the one you trusted. This can mean the host was reinstalled, or that something sits between you and it. Capstan will not connect.")
                    .font(TypeStyle.body)
                if let pin = model.trustedPin(hostId) {
                    FingerprintBlock(label: "Trusted", fingerprint: pin.fingerprint)
                }
                Text("If you reinstalled the host, compare its new fingerprint on the host (ssh-keygen -lf), then remove the host here and add it again to trust the new key.")
                    .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
            .padding(.horizontal, Tokens.margin)
        }
        .background(Tokens.color(.ground))
        .safeAreaInset(edge: .bottom) {
            ActionBar {
                Button("Keep blocked") { dismiss() }.buttonStyle(.capstanQuiet)
                Button("Remove host…") { confirmingRemove = true }.buttonStyle(.capstanDestructive)
            }
        }
        .sheet(isPresented: $confirmingRemove) {
            ConfirmSheet(
                title: "Remove this host?", detail: nil, noteLimit: nil, destructiveLabel: "Remove host",
                onConfirm: { _ in
                    model.removeHost(hostId)
                    dismiss()
                })
        }
    }
}

#Preview("Trust sheet") {
    Color.clear.sheet(isPresented: .constant(true)) {
        TrustSheet(
            pin: HostKeyPin(algorithm: "ssh-ed25519", blob: Data(repeating: 7, count: 51)),
            hostDisplay: "build-host.lan:22", onTrust: {}, onCancel: {})
    }
}

#Preview("Host key changed") {
    NavigationStack { HostKeyChangedView(model: AppModel(preview: PreviewData.changedKey), hostId: PreviewData.betaId) }
}

#Preview("Host key changed, dark") {
    NavigationStack { HostKeyChangedView(model: AppModel(preview: PreviewData.changedKey), hostId: PreviewData.betaId) }
        .preferredColorScheme(.dark)
}
