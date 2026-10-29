import CapstanAppCore
import CapstanKit
import SwiftUI

/// One row of the inbox: a kind glyph, `project · actor` in bold, a second line and the age at the right.
/// The row is flat; only the selection is filled, in the accent.
struct InboxRowView: View {
    let row: InboxRow
    let forwarded: Bool
    let selected: Bool
    /// A quiet row (a merge conflict) is not an action waiting for the person.
    var quiet = false

    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            Image(systemName: row.kind.glyph)
                .frame(width: 20)
                .foregroundStyle(selected ? Tokens.color(.accent) : Tokens.color(.label2))
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: RowText.title(row))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(quiet ? Tokens.color(.label2) : Tokens.color(.label))
                    .lineLimit(1)
                secondLine
            }
            Spacer(minLength: Tokens.space[0])
            TimelineView(.periodic(from: .now, by: 30)) { context in
                Text(verbatim: AgeText.short(since: row.since, now: context.date))
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(.vertical, Tokens.space[0] + 2)
        .padding(.horizontal, Tokens.space[1])
        .background(
            RoundedRectangle(cornerRadius: Tokens.radius.row)
                .fill(selected ? Tokens.color(.accent).opacity(0.16) : (hovering ? Tokens.color(.fill) : Color.clear))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Tokens.radius.row)
                .strokeBorder(Tokens.color(.accent), lineWidth: selected ? 1 : 0)
        )
        .opacity(row.stale ? 0.55 : 1)
        .contentShape(Rectangle())
        .onHover { inside in
            withAnimation(Motion.animation(Motion.hover, reduce: reduceMotion)) { hovering = inside }
        }
        .accessibilityElement(children: .combine)
    }

    private var secondLine: some View {
        HStack(spacing: Tokens.space[0] + 2) {
            ForEach(pills, id: \.word) { pill in
                StatePill(word: pill.word, symbol: symbol(pill), tone: tone(pill))
            }
            switch RowText.secondary(row) {
            case .mono(let text):
                Text(verbatim: text).font(.system(size: 12, design: .monospaced)).lineLimit(1)
            case .plain(let text):
                Text(verbatim: text).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
            case .unverified:
                Text("Text could not be verified").font(.system(size: 13)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
            }
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
}

extension WaitingKind {
    /// The SF Symbol of the kind, in rows and inspectors.
    var glyph: String {
        switch self {
        case .op: return "terminal"
        case .prompt: return "shield"
        case .design: return "photo"
        case .message: return "bubble.left"
        case .integration: return "arrow.triangle.branch"
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
