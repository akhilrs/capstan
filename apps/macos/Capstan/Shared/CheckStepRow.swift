import CapstanAppCore
import CapstanKit
import SwiftUI

enum CheckStepState {
    case pending, running, passed, warning, failed
}

/// One step of a connection check: a glyph for its state, its title and the host's detail when there is one.
struct CheckStepRow: View {
    let title: String
    let state: CheckStepState
    var detail: UntrustedText?

    init(title: String, state: CheckStepState, detail: UntrustedText? = nil) {
        self.title = title
        self.state = state
        self.detail = detail
    }

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            glyph.frame(width: 16)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: title).font(.system(size: 13))
                if let detail {
                    if detail.verified {
                        Text(verbatim: TextBudget.displayPrefix(detail.displayPlain, maxBytes: ProtocolLimits.reasonCap).text)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(Tokens.color(.label2))
                            .textSelection(.enabled)
                    } else {
                        Text("The host could not vouch for this text, so it is not shown.")
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private var glyph: some View {
        switch state {
        case .pending: Image(systemName: "circle").foregroundStyle(Tokens.color(.label2))
        case .running: ProgressView().controlSize(.small)
        case .passed: Image(systemName: "checkmark.circle.fill").foregroundStyle(Tokens.color(.green))
        case .warning: Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Tokens.color(.yellow))
        case .failed: Image(systemName: "xmark.octagon.fill").foregroundStyle(Tokens.color(.red))
        }
    }
}

#Preview("CheckStepRow") {
    VStack(alignment: .leading) {
        CheckStepRow(title: "Connect", state: .pending)
        CheckStepRow(title: "Capstan answers", state: .running)
        CheckStepRow(title: "Projects found", state: .passed)
        CheckStepRow(title: "First snapshot", state: .warning, detail: UntrustedText(raw: "2 of 3 projects", verified: true, truncated: false))
        CheckStepRow(title: "Connect", state: .failed, detail: UntrustedText(raw: "Permission denied (publickey)", verified: true, truncated: false))
    }
    .padding()
}
