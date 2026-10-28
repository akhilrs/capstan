import CapstanAppCore
import CapstanKit
import SwiftUI

/// A strip that says something about the state of the app, with an optional action.
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
            Image(systemName: symbol).foregroundStyle(tone.color)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: text).font(.system(size: 13, weight: .medium))
                if let detail {
                    if detail.verified {
                        Text(verbatim: TextBudget.displayPrefix(detail.displayPlain, maxBytes: ProtocolLimits.reasonCap).text)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(Tokens.color(.label2))
                            .textSelection(.enabled)
                    } else {
                        Text("The host could not vouch for this text, so it is not shown.")
                            .font(.system(size: 12))
                            .foregroundStyle(Tokens.color(.label2))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let actionTitle, let action {
                Button(action: action) { Text(verbatim: actionTitle) }
            }
        }
        .padding(Tokens.space[2])
        .background(tone.color.opacity(0.12), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
        .overlay(RoundedRectangle(cornerRadius: Tokens.radius.row).strokeBorder(tone.color.opacity(0.4), lineWidth: 1))
    }

    private var symbol: String {
        switch tone {
        case .red: return "xmark.octagon.fill"
        case .yellow: return "exclamationmark.triangle.fill"
        case .green: return "checkmark.circle.fill"
        case .blue: return "info.circle.fill"
        case .neutral, .grey: return "circle.fill"
        }
    }
}

#Preview("Banner") {
    VStack {
        Banner(tone: .red, text: "Connection failed", detail: UntrustedText(raw: "ssh: exit 255", verified: true, truncated: false), actionTitle: "Retry", action: {})
        Banner(tone: .yellow, text: "Resyncing")
        Banner(tone: .blue, text: "Forwarded")
        Banner(tone: .green, text: "Live")
        Banner(tone: .grey, text: "Stopped")
    }
    .padding()
    .frame(width: 480)
}
