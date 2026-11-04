import CapstanAppCore
import CapstanKit
import SwiftUI

/// The sheet before Cancel plan… and Cancel draft…: what is cancelled, its title as host text, and the choice. Return
/// and Esc keep the plan; the cancel button is the destructive one.
struct CancelPlanSheet: View {
    let projectName: String
    let planId: String
    let title: UntrustedText
    let stateLine: String
    let isDraft: Bool
    let onConfirm: () -> Void

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            HStack(alignment: .top, spacing: Tokens.space[2]) {
                Image(systemName: "xmark.circle").font(.system(size: 28)).foregroundStyle(Tokens.color(.red))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: isDraft ? "Cancel this draft?" : "Cancel this plan?")
                        .font(.system(size: 15, weight: .semibold))
                    Text(verbatim: isDraft
                        ? "The draft is withdrawn and the PM is told. No package has started. This cannot be taken back from here."
                        : "Its open packages stop and their developers and the PM are told. A cancelled plan cannot be taken back from here.")
                        .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            MetaList(rows: [
                MetaRow(label: "Plan", value: UntrustedText(raw: projectName + " · " + planId, verified: true, truncated: false)),
                MetaRow(label: "State", value: UntrustedText(raw: stateLine, verified: true, truncated: false)),
            ])
            UntrustedTextBlock(text: title, label: "Title · untrusted text")
            HStack {
                Text("esc or return keeps it").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                Spacer()
                Button { dismiss() } label: { Text(verbatim: isDraft ? "Keep draft" : "Keep plan") }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.capstanPrimary)
                Button(role: .destructive) {
                    onConfirm()
                    dismiss()
                } label: {
                    Text(verbatim: isDraft ? "Cancel draft" : "Cancel plan").foregroundStyle(Tokens.color(.red))
                }
            }
        }
        .padding(Tokens.space[4])
        .frame(width: 440)
    }
}

#Preview("Cancel plan") {
    CancelPlanSheet(
        projectName: "payments-api", planId: "plan-15",
        title: UntrustedText(raw: "Refund ledger", verified: true, truncated: false),
        stateLine: "approved · 3 packages · 2 assigned", isDraft: false, onConfirm: {})
}
