import CapstanAppCore
import CapstanKit
import SwiftUI

/// The state of one decision in the words of OutcomePresentation. nil means it is being sent.
struct OutcomeView: View {
    let outcome: DecideOutcome?
    let hostName: String
    let onRefresh: () -> Void
    let onRetry: (UUID) -> Void
    let onTryAgain: () -> Void

    var body: some View {
        let presentation = Presentation.present(outcome, hostName: hostName)
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[1]) {
                if outcome == nil {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: symbol(presentation.tone)).foregroundStyle(presentation.tone.color)
                }
                Text(verbatim: presentation.title).font(.system(size: 13, weight: .semibold))
                Spacer()
                ForEach(Array(presentation.actions.enumerated()), id: \.offset) { _, action in
                    switch action {
                    case .refresh: Button("Refresh", action: onRefresh).buttonStyle(.capstanPrimary)
                    case .retry:
                        if let id = pendingId { Button("Retry") { onRetry(id) }.buttonStyle(.capstanPrimary) }
                    case .tryAgain: Button("Try again", action: onTryAgain).buttonStyle(.capstanQuiet)
                    }
                }
            }
            if let detail {
                UntrustedTextBlock(text: detail, label: "From the host")
            }
        }
        .padding(Tokens.space[2])
        .background(
            (presentation.tone == .neutral || presentation.tone == .grey ? Tokens.color(.fill2) : presentation.tone.color.opacity(0.12)),
            in: RoundedRectangle(cornerRadius: Tokens.radius.block)
        )
        .accessibilityElement(children: .contain)
    }

    private var pendingId: UUID? {
        switch outcome {
        case .notSent(_, let id)?: return id
        case .malformed(_, let id)?: return id
        default: return nil
        }
    }

    /// Host words that explain a refusal or a failed connection.
    private var detail: UntrustedText? {
        switch outcome {
        case .refused(let refusal, _)?: return refusal.message.raw.isEmpty && refusal.message.verified ? nil : refusal.message
        case .notSent(.ssh(let tail), _)?: return tail
        default: return nil
        }
    }

    private func symbol(_ tone: Tone) -> String {
        switch tone {
        case .green: return "checkmark"
        case .blue: return "arrow.right"
        case .yellow: return "arrow.clockwise"
        case .red: return "exclamationmark.circle"
        case .grey, .neutral: return "checkmark.circle"
        }
    }
}

#if DEBUG
private enum OutcomeSamples {
    static func result(_ outcome: DecideKind, isRepeat: Bool = false) -> DecideResult {
        DecideResult(
            decisionId: "d1", requestId: "req-1", nonce: "n1", outcome: outcome, isRepeat: isRepeat, kind: "op",
            verb: "deny", contentHash: "aaaaaaaaaaaa")
    }

    static func refusal(_ code: DecideErrorCode, _ hint: RefusalHint) -> DecideOutcome {
        .refused(DecideRefusal(code: code, message: UntrustedText(raw: "Host said no", verified: true, truncated: false)), hint: hint)
    }

    static let all: [(String, DecideOutcome?)] = [
        ("Sending", nil),
        ("Applied", .applied(result(.applied))),
        ("Applied, repeat", .applied(result(.applied, isRepeat: true))),
        ("Forwarded", .forwarded(result(.forwarded))),
        ("Forwarded, repeat", .forwarded(result(.forwarded, isRepeat: true))),
        ("Needs terminal", refusal(.needsTerminal, .terminal)),
        ("Hash mismatch", refusal(.hashMismatch, .refresh)),
        ("Not waiting", refusal(.notWaiting, .gone)),
        ("No active PM", refusal(.noActivePm, .noPm)),
        ("Limit reached", refusal(.limitReached, .limit)),
        ("Internal", refusal(.other("internal"), .internal)),
        ("Invalid", refusal(.invalidRequest, .invalid)),
        ("Host code", refusal(.forbidden, .host)),
        ("Generic", refusal(.other("odd"), .generic)),
        ("Busy", LocalOutcome.busy()),
        ("Not sent, ssh", .notSent(reason: .ssh(tail: UntrustedText(raw: "ssh: exit 255", verified: true, truncated: false)), pendingId: UUID())),
        ("Not sent, timeout", .notSent(reason: .timeout, pendingId: UUID())),
        ("Malformed", .malformed("Unexpected answer", pendingId: UUID())),
    ]
}

#Preview("OutcomeView, every case") {
    ScrollView {
        VStack(spacing: Tokens.space[1]) {
            ForEach(Array(OutcomeSamples.all.enumerated()), id: \.offset) { _, sample in
                OutcomeView(outcome: sample.1, hostName: "Studio", onRefresh: {}, onRetry: { _ in }, onTryAgain: {})
            }
        }
        .padding()
    }
    .frame(width: 380, height: 720)
}
#endif
