import CapstanAppCore
import CapstanKit
import SwiftUI

/// The exact options of a permission prompt (board 04), in the host's order and words. A selectable option can be
/// chosen and is then sent to the PM, who confirms with the person; an option that widens permissions or takes typed
/// text is shown, flagged and terminal only. Choosing here sends nothing: the action bar does, after a confirmation.
struct PromptOptionList: View {
    let rows: [OptionRow]
    @Binding var selected: Int64?
    /// Selectable rows can be chosen (the item can be answered from here).
    let enabled: Bool

    var body: some View {
        VStack(spacing: Tokens.space[2]) {
            ForEach(rows) { row in
                if row.state == .selectable {
                    Button {
                        if selected != row.number {
                            selected = row.number
                            Haptics.play(.selection)
                        }
                    } label: {
                        OptionCell(row: row, selected: selected == row.number)
                    }
                    .buttonStyle(.plain)
                    .disabled(!enabled)
                    .accessibilityAddTraits(selected == row.number ? .isSelected : [])
                } else {
                    OptionCell(row: row, selected: false)
                }
            }
        }
    }
}

/// True when no option can be sent from the phone: every one widens permissions or takes text.
extension Array where Element == OptionRow {
    var allTerminal: Bool { !isEmpty && allSatisfy { $0.state != .selectable } }
    var firstSelectable: Int64? { first { $0.state == .selectable }?.number }
}

private struct OptionCell: View {
    let row: OptionRow
    let selected: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[3]) {
            Text(verbatim: String(row.number))
                .font(TypeStyle.mono)
                .foregroundStyle(Tokens.color(.label2))
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text(verbatim: shownText)
                    .font(TypeStyle.mono)
                    .foregroundStyle(Tokens.color(.label))
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                caption
            }
            if selected {
                Image(systemName: "checkmark.circle")
                    .foregroundStyle(Tokens.color(.accent))
                    .accessibilityHidden(true)
            }
        }
        .padding(Tokens.space[3])
        .frame(minHeight: 44)
        .background(background, in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        .overlay(
            RoundedRectangle(cornerRadius: Tokens.radius.section)
                .strokeBorder(selected ? Tokens.color(.accent) : Color.clear, lineWidth: 2))
        .contentShape(RoundedRectangle(cornerRadius: Tokens.radius.section))
        .accessibilityElement(children: .combine)
    }

    private var shownText: String {
        TextBudget.displayPrefix(row.text.displayPlain, maxBytes: ProtocolLimits.optionTextCap).text
    }

    @ViewBuilder
    private var caption: some View {
        switch row.state {
        case .selectable:
            if selected {
                Text("Sent to the PM, who confirms with you")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
            }
        case .widening:
            HStack(spacing: Tokens.space[0]) {
                Image(systemName: "exclamationmark.triangle").imageScale(.small).foregroundStyle(Tokens.color(.red))
                Text("Widens permissions").font(TypeStyle.footnote.weight(.semibold)).foregroundStyle(Tokens.color(.red))
                Text("· terminal only").font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
        case .text:
            Text("Takes text · terminal only")
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
        }
    }

    private var background: Color {
        switch row.state {
        case .widening: return Tokens.color(.red).opacity(0.1)
        case .selectable: return selected ? Tokens.color(.accent).opacity(0.08) : Tokens.color(.cell)
        case .text: return Tokens.color(.cell)
        }
    }
}

#Preview("PromptOptionList") {
    @Previewable @State var selected: Int64? = 1
    let text = { (s: String) in UntrustedText(raw: s, verified: true, truncated: false) }
    ScrollView {
        PromptOptionList(
            rows: [
                OptionRow(number: 1, text: text("Yes"), state: .selectable),
                OptionRow(number: 2, text: text("Yes, and don't ask again for rm commands in this project"), state: .widening),
                OptionRow(number: 3, text: text("No, and tell the agent what to do differently"), state: .text),
            ],
            selected: $selected, enabled: true)
        .padding()
    }
    .background(Tokens.color(.ground))
}
