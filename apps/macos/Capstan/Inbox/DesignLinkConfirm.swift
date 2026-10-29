import AppKit
import CapstanAppCore
import CapstanKit
import SwiftUI

/// Shows the whole address of a design link; the page opens only after the person confirms it and the address is http or https.
struct DesignLinkConfirm: View {
    let link: UntrustedText

    @Environment(\.dismiss) private var dismiss

    private var url: URL? { link.verified ? LinkCheck.openable(link.raw) : nil }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text("Open this link?").font(.system(size: 15, weight: .semibold))
            UntrustedTextBlock(text: link, label: "Full address")
            if url == nil {
                Text("Only http and https addresses can be opened from here.")
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.red))
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Open in browser") {
                    if let url { NSWorkspace.shared.open(url) }
                    dismiss()
                }
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(url == nil)
            }
        }
        .padding(Tokens.space[4])
        .frame(width: 420)
    }
}

#Preview("DesignLinkConfirm") {
    VStack {
        DesignLinkConfirm(link: UntrustedText(raw: "https://example.org/design/42", verified: true, truncated: false))
        DesignLinkConfirm(link: UntrustedText(raw: "javascript:alert(1)", verified: true, truncated: false))
        DesignLinkConfirm(link: UntrustedText(raw: "", verified: false, truncated: false))
    }
}
