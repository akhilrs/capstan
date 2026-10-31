import CapstanAppCore
import CapstanKit
import SwiftUI

/// The top of a details screen: the kind glyph with "Kind · project", the title, and the row's pills.
struct DetailHeader: View {
    let kind: WaitingKind
    let projectName: String
    let title: String
    let pills: [Pill]

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: kind.glyph)
                    .foregroundStyle(Tokens.color(.accent))
                    .accessibilityHidden(true)
                Text(verbatim: RowText.kindName(kind) + " · " + projectName)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
            }
            Text(verbatim: title)
                .font(TypeStyle.title2)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            if !pills.isEmpty {
                HStack(spacing: Tokens.space[1]) {
                    ForEach(pills, id: \.word) { pill in
                        StatePill(word: pill.word, symbol: WaitingRow.symbol(pill), tone: WaitingRow.tone(pill))
                    }
                }
            }
        }
    }
}

/// One fact of an item: ids, names and times the app reads from the snapshot. Host text goes in `DetailTextBlock`.
struct DetailFact: Identifiable {
    let label: String
    let value: String
    var mono = false
    var id: String { label }
}

/// The facts of an item in a grouped cell: label and value side by side, one column at accessibility sizes.
struct DetailFacts: View {
    let facts: [DetailFact]
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        Group {
            if typeSize.isAccessibilitySize {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    ForEach(facts) { fact in
                        VStack(alignment: .leading, spacing: Tokens.space[0]) {
                            label(fact)
                            value(fact)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            } else {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: Tokens.space[3], verticalSpacing: Tokens.space[1]) {
                    ForEach(facts) { fact in
                        GridRow {
                            label(fact).gridColumnAlignment(.leading)
                            value(fact)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, Tokens.space[3])
        .padding(.vertical, Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
    }

    private func label(_ fact: DetailFact) -> some View {
        Text(verbatim: fact.label).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
    }

    private func value(_ fact: DetailFact) -> some View {
        Text(verbatim: fact.value)
            .font(fact.mono ? TypeStyle.monoSmall : TypeStyle.subheadline)
            .foregroundStyle(Tokens.color(.label))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Times as the details screens show them: the clock time on this phone and the age, "12:45, 2 min ago".
enum DetailTime {
    static func asked(_ since: String, now: Date) -> String {
        guard let date = parse(since) else { return since }
        let clock = date.formatted(date: .omitted, time: .shortened)
        let age = AgeText.short(since: since, now: now)
        if age.isEmpty { return clock }
        return age == "now" ? clock + ", just now" : clock + ", " + age + " ago"
    }

    static func parse(_ text: String) -> Date? {
        let plain = ISO8601DateFormatter()
        if let date = plain.date(from: text) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text)
    }
}

#Preview("DetailHeader") {
    VStack(alignment: .leading, spacing: Tokens.space[3]) {
        DetailHeader(kind: .prompt, projectName: "docs-site", title: "An agent asks for permission", pills: [.forwarded])
        DetailFacts(facts: [
            DetailFact(label: "Agent", value: "dev-3 · developer"),
            DetailFact(label: "Asked", value: "12:45, 2 min ago"),
            DetailFact(label: "Request", value: "prompt:31 · 3c09e1d47a2b", mono: true),
        ])
    }
    .padding()
    .background(Tokens.color(.ground))
}
