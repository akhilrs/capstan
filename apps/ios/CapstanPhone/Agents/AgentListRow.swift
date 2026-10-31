import CapstanAppCore
import CapstanKit
import SwiftUI

/// One agent in the list: its id and role, its health as a pill with a word and glyph and how long the host has
/// seen it, then its task and branch on one or two lines. Flat: rows are never glass.
struct AgentListRow: View {
    let row: AgentRow

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Text(verbatim: row.agentId).font(TypeStyle.headline)
                Text(verbatim: row.role).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                Spacer(minLength: Tokens.space[1])
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    StatePill(word: pillWord(now: context.date), symbol: row.healthSymbol, tone: AgentTone.tone(row.health))
                }
            }
            secondLine
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var secondLine: some View {
        if let task = row.task {
            Text(verbatim: row.branch.map { task + " · " + $0 } ?? task)
                .font(TypeStyle.subheadline)
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(2)
        } else if row.taskUnverified {
            Text("Task not shown: it failed its integrity check")
                .font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
        } else {
            Text("No task").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
        }
    }

    /// The health word and, when the host said since when, how long: "Working 1 h".
    private func pillWord(now: Date) -> String {
        guard let since = row.source.activitySince else { return row.healthWord }
        let age = AgeText.short(since: since, now: now)
        return age.isEmpty || age == "now" ? row.healthWord : row.healthWord + " " + age
    }
}

/// The tone of a health, as on the Mac; the word and glyph always go with it.
enum AgentTone {
    static func tone(_ health: AgentHealth) -> Tone {
        switch health {
        case .working: return .green
        case .needsYou: return .accent
        case .stalled: return .yellow
        case .lost, .blocked: return .red
        case .paused, .idle: return .grey
        }
    }
}
