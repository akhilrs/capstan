import CapstanAppCore
import CapstanKit
import SwiftUI

/// A strip that says something about the state of the app, with an optional action. Flat and tonal: banners are
/// content, never glass.
struct Banner: View {
    let tone: Tone
    let text: String
    var detail: UntrustedText?
    var actionTitle: String?
    var action: (() -> Void)?

    init(
        tone: Tone, text: String, detail: UntrustedText? = nil, actionTitle: String? = nil,
        action: (() -> Void)? = nil
    ) {
        self.tone = tone
        self.text = text
        self.detail = detail
        self.actionTitle = actionTitle
        self.action = action
    }

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            Image(systemName: symbol).foregroundStyle(tone.color).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: text).font(TypeStyle.subheadline.weight(.semibold))
                if let detail {
                    if detail.verified {
                        Text(verbatim: TextBudget.displayPrefix(detail.displayPlain, maxBytes: ProtocolLimits.reasonCap).text)
                            .font(TypeStyle.monoSmall)
                            .foregroundStyle(Tokens.color(.label2))
                            .textSelection(.enabled)
                    } else {
                        Text("The host could not vouch for this text, so it is not shown.")
                            .font(TypeStyle.footnote)
                            .foregroundStyle(Tokens.color(.label2))
                    }
                }
                if let actionTitle, let action {
                    Button(action: action) { Text(verbatim: actionTitle) }
                        .font(TypeStyle.subheadline.weight(.semibold))
                        .foregroundStyle(Tokens.color(.accent))
                        .frame(minHeight: 44)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[2])
        .background(tone.color.opacity(0.12), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }

    private var symbol: String {
        switch tone {
        case .red: return "exclamationmark.circle"
        case .yellow: return "wifi"
        case .green: return "checkmark.circle"
        case .blue, .accent: return "arrow.clockwise"
        case .neutral, .grey: return "info.circle"
        }
    }
}

#Preview("Banner") {
    VStack {
        Banner(tone: .red, text: "Connection failed", detail: UntrustedText(raw: "ssh: exit 255", verified: true, truncated: false), actionTitle: "Try again", action: {})
        Banner(tone: .yellow, text: "Reconnecting")
        Banner(tone: .grey, text: "Stopped")
    }
    .padding()
}
