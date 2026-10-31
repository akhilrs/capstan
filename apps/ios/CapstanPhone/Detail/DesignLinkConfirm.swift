import CapstanAppCore
import CapstanKit
import SwiftUI

/// The only place in the app that opens a link. It shows the whole address of a design link first (open question 4:
/// the default is to show it), and opens it in Safari only after the person confirms and only when it is an http or
/// https address with a host. Cancel is the default.
struct DesignLinkConfirm: View {
    let link: UntrustedText

    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(\.dynamicTypeSize) private var typeSize
    @FocusState private var cancelFocused: Bool

    private var url: URL? { link.verified ? LinkCheck.openable(link.raw) : nil }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text("Open this link in Safari?").font(TypeStyle.title2)
                DetailTextBlock(text: link, name: "Full address", shownAs: "shown as text", symbol: "link")
                if url == nil {
                    Text("Only http and https addresses can be opened from here.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.red))
                } else {
                    Text("Check the address before you open it. Capstan does not open links on its own.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                }
                if typeSize.isAccessibilitySize {
                    VStack(spacing: Tokens.space[2]) { buttons }
                } else {
                    HStack(spacing: Tokens.space[2]) { buttons }
                }
            }
            .padding(Tokens.margin)
        }
        .presentationDetents([.medium, .large])
        .presentationBackground(Tokens.color(.cell))
        .onAppear { cancelFocused = true }
    }

    @ViewBuilder
    private var buttons: some View {
        Button("Cancel") { dismiss() }
            .buttonStyle(.capstanQuiet)
            .keyboardShortcut(.cancelAction)
            .focused($cancelFocused)
        Button("Open in Safari") {
            if let url { openURL(url) }
            dismiss()
        }
        .buttonStyle(.capstanPrimary)
        .disabled(url == nil)
    }
}

#Preview("DesignLinkConfirm") {
    Color.clear.sheet(isPresented: .constant(true)) {
        DesignLinkConfirm(link: UntrustedText(raw: "https://claude.ai/artifact/7Qe-example-Lm2", verified: true, truncated: false))
    }
}

#Preview("DesignLinkConfirm, not http") {
    Color.clear.sheet(isPresented: .constant(true)) {
        DesignLinkConfirm(link: UntrustedText(raw: "javascript:alert(1)", verified: true, truncated: false))
    }
}
