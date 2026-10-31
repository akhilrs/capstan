import CapstanAppCore
import CapstanKit
import SwiftUI

/// The result of a decision on a details screen (board 05): a tonal card with the glyph, the title from
/// `Presentation`, the line from `OutcomeLines`, host words as plain text, and Retry or Try again when they apply.
/// Refresh is the action bar's "Show the new version". Flat content, never glass.
struct OutcomeBanner: View {
    let outcome: DecideOutcome
    let hostName: String
    let receivedAt: Date
    var onRetry: ((UUID) -> Void)?
    var onTryAgain: (() -> Void)?

    var body: some View {
        let presentation = Presentation.present(outcome, hostName: hostName)
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            Image(systemName: OutcomeLines.symbol(outcome))
                .foregroundStyle(glyphColor(presentation.tone))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text(verbatim: presentation.title).font(TypeStyle.subheadline.weight(.semibold))
                if let line = OutcomeLines.detail(outcome, at: receivedAt) {
                    Text(verbatim: line).font(TypeStyle.subheadline).fixedSize(horizontal: false, vertical: true)
                }
                if let host = OutcomeLines.hostText(outcome) {
                    if host.verified {
                        Text(verbatim: TextBudget.displayPrefix(host.displayPlain, maxBytes: ProtocolLimits.reasonCap).text)
                            .font(TypeStyle.monoSmall)
                            .foregroundStyle(Tokens.color(.label2))
                            .textSelection(.enabled)
                    } else {
                        Text("The host's words failed their integrity check.")
                            .font(TypeStyle.footnote)
                            .foregroundStyle(Tokens.color(.label2))
                    }
                }
                actions(presentation)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[3])
        .background(background(presentation.tone), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private func actions(_ presentation: Presentation) -> some View {
        let pending = OutcomeLines.pendingId(outcome)
        let retry = presentation.actions.contains(.retry) && pending != nil && onRetry != nil
        let again = presentation.actions.contains(.tryAgain) && onTryAgain != nil
        if retry || again {
            HStack(spacing: Tokens.space[2]) {
                if retry, let pending, let onRetry {
                    Button("Retry") { onRetry(pending) }
                        .buttonStyle(.capstanQuiet)
                        .fixedSize()
                }
                if again, let onTryAgain {
                    Button("Try again") { onTryAgain() }
                        .buttonStyle(.capstanQuiet)
                        .fixedSize()
                }
            }
            .padding(.top, Tokens.space[1])
        }
    }

    private func glyphColor(_ tone: Tone) -> Color {
        tone == .grey || tone == .neutral ? Tokens.color(.label2) : tone.color
    }

    private func background(_ tone: Tone) -> Color {
        tone == .grey || tone == .neutral ? Tokens.color(.fill2) : tone.color.opacity(0.14)
    }
}

/// The primary button's label while a decision is on its way.
struct SendingLabel: View {
    var body: some View {
        Label("Sending…", systemImage: "clock")
    }
}

#if DEBUG
enum OutcomeSamples {
    static func result(_ kind: DecideKind, isRepeat: Bool = false) -> DecideResult {
        DecideResult(
            decisionId: "d1", requestId: "req-1", nonce: "n1", outcome: kind, isRepeat: isRepeat, kind: "op",
            verb: "deny", contentHash: "fa78fb90bf4b")
    }

    static func refused(_ wire: String) -> DecideOutcome {
        let code = DecideErrorCode(wire: wire)
        return .refused(
            DecideRefusal(code: code, message: UntrustedText(raw: "", verified: true, truncated: false)),
            hint: RefusalHint(code: code))
    }

    /// Every result the host can answer, in the order of board 05.
    static var all: [DecideOutcome] {
        [
            .applied(result(.applied)),
            .forwarded(result(.forwarded)),
            refused("needs_terminal"),
            refused("not_waiting"),
            .applied(result(.applied, isRepeat: true)),
            refused("no_active_pm"),
            refused("limit_reached"),
            refused("hash_mismatch"),
            .notSent(
                reason: .ssh(tail: UntrustedText(raw: "ssh: connect to host build-host port 22: Operation timed out", verified: true, truncated: false)),
                pendingId: UUID()),
            .malformed("bad json", pendingId: UUID()),
        ]
    }
}

#Preview("Every outcome") {
    ScrollView {
        VStack(spacing: Tokens.space[2]) {
            ForEach(Array(OutcomeSamples.all.enumerated()), id: \.offset) { _, outcome in
                OutcomeBanner(outcome: outcome, hostName: "build-host", receivedAt: Date(), onRetry: { _ in }, onTryAgain: {})
            }
        }
        .padding()
    }
    .background(Tokens.color(.ground))
}

#Preview("Every outcome, dark") {
    ScrollView {
        VStack(spacing: Tokens.space[2]) {
            ForEach(Array(OutcomeSamples.all.enumerated()), id: \.offset) { _, outcome in
                OutcomeBanner(outcome: outcome, hostName: "build-host", receivedAt: Date(), onRetry: { _ in }, onTryAgain: {})
            }
        }
        .padding()
    }
    .background(Tokens.color(.ground))
    .preferredColorScheme(.dark)
}
#endif
