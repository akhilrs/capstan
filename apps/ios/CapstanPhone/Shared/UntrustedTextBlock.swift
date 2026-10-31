import CapstanAppCore
import CapstanKit
import SwiftUI

/// Host text in a flat mono block with a labelled header. Plain text only: no markup, no link, no glass.
struct UntrustedTextBlock: View {
    let text: UntrustedText
    let label: String

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[0]) {
                Image(systemName: "exclamationmark.triangle").imageScale(.small)
                Text(verbatim: label + ", untrusted text").font(TypeStyle.footnote.weight(.semibold))
            }
            .foregroundStyle(Tokens.color(.label2))
            Divider()
            if !text.verified {
                Text("The host could not vouch for this text, so it is not shown.")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
            } else {
                let shown = TextBudget.displayPrefix(text.displayPlain, maxBytes: ProtocolLimits.textCap)
                Text(verbatim: shown.text)
                    .font(TypeStyle.mono)
                    .foregroundStyle(Tokens.color(.label))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if text.truncated || shown.cut {
                    Text("The text was cut to its cap.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                }
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .combine)
    }
}

#Preview("UntrustedTextBlock") {
    VStack(spacing: Tokens.space[2]) {
        UntrustedTextBlock(text: UntrustedText(raw: "git push origin main", verified: true, truncated: false), label: "Command")
        UntrustedTextBlock(text: UntrustedText(raw: "cut by the host", verified: true, truncated: true), label: "Cut")
        UntrustedTextBlock(text: UntrustedText(raw: "", verified: false, truncated: false), label: "Failed integrity")
    }
    .padding()
    .background(Tokens.color(.ground))
}
