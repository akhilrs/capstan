import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// One row of the Waiting list (board 02): the kind glyph, `project · actor`, the age, and a second line with the
/// forwarded, terminal-only, stale and widening pills before the plain summary. Flat: rows are never glass.
struct WaitingRow: View {
    let row: InboxRow
    let forwarded: Bool
    /// How many of a prompt's options widen permissions.
    let widening: Int
    var quiet = false
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            Image(systemName: row.set.map { SetText.state($0).symbol } ?? row.kind.glyph)
                .foregroundStyle(Tokens.color(.label2))
                .frame(width: 24)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                // At accessibility sizes the title wraps and the age goes under it.
                let layout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: Tokens.space[0]))
                    : AnyLayout(HStackLayout(alignment: .firstTextBaseline))
                layout {
                    Text(verbatim: RowText.title(row))
                        .font(TypeStyle.headline)
                        .foregroundStyle(quiet ? Tokens.color(.label2) : Tokens.color(.label))
                        // A set title wraps to two lines (board 08); other rows keep one.
                        .lineLimit(typeSize.isAccessibilitySize ? 3 : (row.set == nil ? 1 : 2))
                    if !typeSize.isAccessibilitySize { Spacer(minLength: Tokens.space[1]) }
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        Text(verbatim: AgeText.short(since: row.since, now: context.date))
                            .font(TypeStyle.subheadline.monospacedDigit())
                            .foregroundStyle(Tokens.color(.label2))
                    }
                }
                secondLine
            }
        }
        .padding(.vertical, Tokens.space[0])
        .opacity(row.stale ? 0.6 : 1)
        .contentShape(Rectangle())
        .frame(minHeight: 44)
        .accessibilityElement(children: .combine)
        .setVoiceover(row.set.map { SetText.rowVoiceover($0) })
    }

    private var secondLine: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            let pills = RowPills.make(needsTerminal: row.needsTerminal, forwarded: forwarded, stale: row.stale)
            if widening > 0 || !pills.isEmpty {
                let pillLayout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: Tokens.space[1]))
                    : AnyLayout(HStackLayout(spacing: Tokens.space[1]))
                pillLayout {
                    if widening > 0 {
                        Text(verbatim: widening == 1 ? "1 option widens" : String(widening) + " options widen")
                            .font(TypeStyle.subheadline.weight(.medium))
                            .foregroundStyle(Tokens.color(.red))
                    }
                    ForEach(pills, id: \.word) { pill in
                        StatePill(word: pill.word, symbol: WaitingRow.symbol(pill), tone: WaitingRow.tone(pill))
                    }
                }
            }
            if let set = row.set {
                // The strip is 8 glyphs then +n on the phone; the count sentence is the VoiceOver text.
                Text(verbatim: PhoneSetText.secondary(set))
                    .font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2)).lineLimit(3)
                    .accessibilityLabel(Text(verbatim: PhoneSetText.stripVoiceover(set)))
            } else {
            switch RowText.secondary(row) {
            case .mono(let text):
                Text(verbatim: text).font(TypeStyle.mono).lineLimit(2)
            case .plain(let text):
                Text(verbatim: text).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2)).lineLimit(2)
            case .unverified:
                Text("Text could not be verified").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
            }
            }
        }
    }

    static func symbol(_ pill: Pill) -> String {
        switch pill {
        case .forwarded: return "arrow.right.circle.fill"
        case .terminalOnly: return "terminal.fill"
        case .stale: return "clock.fill"
        }
    }

    static func tone(_ pill: Pill) -> Tone {
        switch pill {
        case .forwarded: return .blue
        case .terminalOnly: return .grey
        case .stale: return .yellow
        }
    }
}

extension WaitingKind {
    /// The SF Symbol of the kind, in rows and details.
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

/// A row-shaped placeholder while the first snapshot is read (board 06).
struct SkeletonRow: View {
    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            RoundedRectangle(cornerRadius: 4).fill(Tokens.color(.fill2)).frame(width: 24, height: 20)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                RoundedRectangle(cornerRadius: 4).fill(Tokens.color(.fill2)).frame(width: 200, height: 16)
                RoundedRectangle(cornerRadius: 4).fill(Tokens.color(.fill)).frame(height: 14)
            }
        }
        .padding(.vertical, Tokens.space[2])
        .accessibilityHidden(true)
    }
}

extension View {
    /// Replaces the combined VoiceOver text only when there is one: a set row reads `a11y.set.row`.
    @ViewBuilder
    func setVoiceover(_ label: String?) -> some View {
        if let label { accessibilityLabel(Text(verbatim: label)) } else { self }
    }
}
