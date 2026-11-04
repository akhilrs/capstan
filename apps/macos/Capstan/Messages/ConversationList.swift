import CapstanAppCore
import CapstanKit
import SwiftUI

/// The conversations, newest first: project, host, what was said last, and how many PM questions wait.
struct ConversationList: View {
    let rows: [Conversation]
    let selected: String?
    let select: (String) -> Void

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Tokens.space[0]) {
                ForEach(rows) { row in
                    Button {
                        select(row.id)
                    } label: {
                        ConversationRowView(row: row, selected: row.id == selected)
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(row.id == selected ? .isSelected : [])
                }
            }
            .padding(Tokens.space[1])
        }
    }
}

private struct ConversationRowView: View {
    let row: Conversation
    let selected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[0] + 2) {
                Text(verbatim: row.projectName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                Text(verbatim: "· " + row.hostName).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                Spacer(minLength: Tokens.space[0])
                if row.waiting > 0 {
                    Text(verbatim: String(row.waiting))
                        .font(.system(size: 12, weight: .semibold).monospacedDigit())
                        .foregroundStyle(Tokens.color(.accent))
                        .padding(.horizontal, Tokens.space[0] + 2)
                        .background(Tokens.color(.accent).opacity(0.18), in: Capsule())
                }
                if let at = row.lastAt {
                    Text(verbatim: AgeText.short(since: ISO8601DateFormatter().string(from: at), now: Date()))
                        .font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                }
            }
            Text(verbatim: row.last)
                .font(.system(size: 12))
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(2)
        }
        .padding(.vertical, Tokens.space[0] + 2)
        .padding(.horizontal, Tokens.space[1])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Tokens.radius.control)
                .fill(selected ? Tokens.color(.accent).opacity(0.18) : Color.clear))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}
