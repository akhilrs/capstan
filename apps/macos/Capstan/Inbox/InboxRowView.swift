import CapstanAppCore
import CapstanKit
import SwiftUI

/// One row of the inbox: glyph, project, one line of the summary, the age and the pills.
struct InboxRowView: View {
    let row: InboxRow
    let forwarded: Bool
    let selected: Bool

    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: Tokens.space[1]) {
            Image(systemName: glyph)
                .frame(width: 20)
                .foregroundStyle(Tokens.color(.accent))
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: TextBudget.displayPrefix(row.projectName, maxBytes: 64).text)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                summary
            }
            Spacer(minLength: Tokens.space[0])
            VStack(alignment: .trailing, spacing: 2) {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    Text(verbatim: AgeText.short(since: row.since, now: context.date))
                        .font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                }
                HStack(spacing: Tokens.space[0]) {
                    ForEach(pills, id: \.word) { pill in
                        StatePill(word: pill.word, symbol: symbol(pill), tone: tone(pill))
                    }
                }
            }
        }
        .padding(.vertical, Tokens.space[0])
        .padding(.horizontal, Tokens.space[1])
        .background(
            RoundedRectangle(cornerRadius: Tokens.radius.row)
                .fill(hovering && !selected ? Tokens.color(.fill) : Color.clear)
        )
        .contentShape(Rectangle())
        .onHover { inside in
            withAnimation(Motion.animation(Motion.hover, reduce: reduceMotion)) { hovering = inside }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var summary: some View {
        if row.summary.verified {
            Text(verbatim: TextBudget.displayPrefix(row.summary.displayPlain, maxBytes: 200).text)
                .font(.system(size: 13))
                .lineLimit(1)
        } else {
            Text("Text could not be verified")
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.red))
                .lineLimit(1)
        }
    }

    private var pills: [Pill] {
        RowPills.make(needsTerminal: row.needsTerminal, forwarded: forwarded, stale: row.stale)
    }

    private func symbol(_ pill: Pill) -> String {
        switch pill {
        case .forwarded: return "arrow.right.circle.fill"
        case .terminalOnly: return "terminal.fill"
        case .stale: return "clock.fill"
        }
    }

    private func tone(_ pill: Pill) -> Tone {
        switch pill {
        case .forwarded: return .blue
        case .terminalOnly: return .grey
        case .stale: return .yellow
        }
    }

    private var glyph: String {
        switch row.kind {
        case .op: return "terminal"
        case .prompt: return "hand.raised"
        case .design: return "paintpalette"
        case .message: return "bubble.left"
        case .integration: return "arrow.triangle.merge"
        case .other: return "questionmark.circle"
        }
    }
}

#if DEBUG
#Preview("InboxRowView, every pill and none") {
    let base = InboxRow(
        id: InboxRowID(hostId: PreviewData.alphaId, projectId: "capstan", key: "k"), kind: .op, projectName: "capstan",
        since: "2026-10-29T08:00:00Z", summary: UntrustedText(raw: "git push origin feat/plan-92", verified: true, truncated: false),
        needsTerminal: false, forwarded: false, stale: false, requestId: "r", contentHash: "aaaaaaaaaaaa")
    func variant(terminal: Bool = false, stale: Bool = false, verified: Bool = true) -> InboxRow {
        InboxRow(
            id: base.id, kind: base.kind, projectName: base.projectName, since: base.since,
            summary: UntrustedText(raw: verified ? "git push origin feat/plan-92" : "", verified: verified, truncated: false),
            needsTerminal: terminal, forwarded: false, stale: stale, requestId: "r", contentHash: "aaaaaaaaaaaa")
    }
    return VStack(alignment: .leading) {
        InboxRowView(row: base, forwarded: false, selected: false)
        InboxRowView(row: base, forwarded: true, selected: false)
        InboxRowView(row: variant(terminal: true), forwarded: false, selected: false)
        InboxRowView(row: variant(stale: true), forwarded: false, selected: false)
        InboxRowView(row: variant(terminal: true, stale: true), forwarded: true, selected: true)
        InboxRowView(row: variant(verified: false), forwarded: false, selected: false)
    }
    .padding()
    .frame(width: 420)
}
#endif
