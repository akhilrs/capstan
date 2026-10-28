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
                Image(systemName: "exclamationmark.triangle.fill").imageScale(.small)
                Text(verbatim: label).font(.system(size: 12, weight: .semibold))
            }
            .foregroundStyle(Tokens.color(.label2))
            if !text.verified {
                Text("The host could not vouch for this text, so it is not shown.")
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.red))
            } else {
                let shown = TextBudget.displayPrefix(text.displayPlain, maxBytes: ProtocolLimits.textCap)
                Text(verbatim: shown.text)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(Tokens.color(.label))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if text.truncated || shown.cut {
                    Text("The text was cut to its cap.")
                        .font(.system(size: 12))
                        .foregroundStyle(Tokens.color(.yellow))
                }
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }
}

#Preview("UntrustedTextBlock") {
    let long = String(repeating: "abcdefghij", count: 410).prefix(4096)
    VStack(spacing: Tokens.space[2]) {
        UntrustedTextBlock(text: UntrustedText(raw: "git push origin main", verified: true, truncated: false), label: "Command")
        UntrustedTextBlock(text: UntrustedText(raw: String(long), verified: true, truncated: false), label: "4096 bytes")
        UntrustedTextBlock(text: UntrustedText(raw: "cut by the host", verified: true, truncated: true), label: "Cut")
        UntrustedTextBlock(text: UntrustedText(raw: "", verified: false, truncated: false), label: "Failed integrity")
    }
    .padding()
    .frame(width: 480)
}
