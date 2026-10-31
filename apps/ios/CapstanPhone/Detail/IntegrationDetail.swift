import CapstanAppCore
import CapstanKit
import SwiftUI
import UIKit

/// The body of a branch ready to merge (board 03): the state in words, the facts, and rows that open what it is for,
/// its impact and results, and the next steps. Host text is plain text in flat blocks; nothing here is a link. Merging
/// is done from the terminal until the Merge sheet exists.
struct IntegrationDetail: View {
    let rows: IntegrationDetailRows

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            StatePill(word: rows.stateWord, symbol: stateSymbol(rows.stateTone), tone: rows.stateTone)
            if let lead = rows.lead {
                Text(verbatim: lead)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !rows.meta.isEmpty {
                IntegrationMeta(meta: rows.meta)
            }
            ForEach(Array(rows.banners.enumerated()), id: \.offset) { _, block in
                DetailBlockView(block: block)
            }
            if !rows.sections.isEmpty || !rows.steps.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(rows.sections.enumerated()), id: \.offset) { index, section in
                        if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                        NavigationLink {
                            IntegrationSectionScreen(section: section)
                        } label: {
                            LinkRow(title: section.title, summary: section.summary)
                        }
                    }
                    if !rows.steps.isEmpty {
                        if !rows.sections.isEmpty { Divider().padding(.leading, Tokens.space[3]) }
                        NavigationLink {
                            IntegrationStepsScreen(steps: rows.steps)
                        } label: {
                            LinkRow(title: "Next steps", summary: stepsSummary)
                        }
                    }
                }
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            }
        }
    }

    private var stepsSummary: String {
        rows.steps.count == 1 ? "1 step" : String(rows.steps.count) + " steps"
    }
}

func stateSymbol(_ tone: Tone) -> String {
    switch tone {
    case .yellow: return "exclamationmark.triangle.fill"
    case .green: return "checkmark.circle.fill"
    case .red: return "xmark.octagon.fill"
    case .grey, .neutral: return "info.circle.fill"
    default: return "clock.fill"
    }
}

/// Branch and head of an integration; the branch is host text, so a failed check shows words instead.
private struct IntegrationMeta: View {
    let meta: [DetailMeta]

    var body: some View {
        DetailFacts(facts: meta.map { row in
            if row.value.verified {
                return DetailFact(
                    label: row.label,
                    value: TextBudget.displayPrefix(row.value.displayPlain, maxBytes: ProtocolLimits.snapshotTextCap).text,
                    mono: true)
            }
            return DetailFact(label: row.label, value: "Not shown: it failed its integrity check")
        })
    }
}

private struct LinkRow: View {
    let title: String
    let summary: String?

    var body: some View {
        HStack(spacing: Tokens.space[2]) {
            Text(verbatim: title).font(TypeStyle.body).foregroundStyle(Tokens.color(.label))
            Spacer(minLength: Tokens.space[2])
            if let summary {
                Text(verbatim: summary)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .multilineTextAlignment(.trailing)
            }
            Image(systemName: "chevron.right")
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
                .accessibilityHidden(true)
        }
        .padding(.horizontal, Tokens.space[3])
        .frame(minHeight: 56)
        .contentShape(Rectangle())
    }
}

/// One section of an integration (what it is for, impact, results) on its own screen.
struct IntegrationSectionScreen: View {
    let section: DetailSection

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                if let summary = section.summary {
                    Text(verbatim: summary).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                }
                ForEach(Array(section.blocks.enumerated()), id: \.offset) { _, block in
                    DetailBlockView(block: block)
                }
            }
            .padding(Tokens.margin)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationTitle(
            Text(verbatim: section.title))
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// The next steps of an integration: the commands to run, each with a Copy button.
struct IntegrationStepsScreen: View {
    let steps: [DetailStep]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[4]) {
                ForEach(Array(steps.enumerated()), id: \.offset) { index, step in
                    HStack(alignment: .top, spacing: Tokens.space[2]) {
                        Text(verbatim: String(index + 1))
                            .font(TypeStyle.headline.monospacedDigit())
                            .foregroundStyle(index == 0 ? Tokens.color(.accent) : Tokens.color(.label2))
                            .frame(minWidth: 20, alignment: .trailing)
                        StepBody(step: step)
                    }
                }
            }
            .padding(Tokens.margin)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationTitle("Next steps")
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct StepBody: View {
    let step: DetailStep

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if let title = step.title {
                Text(verbatim: title).font(TypeStyle.headline)
            }
            if let note = step.note {
                Text(verbatim: note)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let command = step.command {
                Text(verbatim: command)
                    .font(TypeStyle.monoSmall)
                    .textSelection(.enabled)
                    .padding(Tokens.space[2])
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
                Button {
                    UIPasteboard.general.string = command
                    Haptics.play(.success)
                } label: {
                    Label("Copy commands", systemImage: "doc.on.doc")
                }
                .buttonStyle(.capstanQuiet)
            }
            if let host = step.hostText {
                DetailTextBlock(text: host.text, name: "From the host")
            }
        }
    }
}

/// One block of an integration section.
struct DetailBlockView: View {
    let block: DetailBlock

    var body: some View {
        switch block {
        case .note(let text):
            Text(verbatim: text)
                .font(TypeStyle.subheadline)
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
        case .banner(let tone, let title, let text):
            DetailNotice(tone: tone, symbol: stateSymbol(tone), title: title, text: text)
        case .stat(let files, let added, let removed):
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: files).font(TypeStyle.headline.monospacedDigit())
                Text(verbatim: added).font(TypeStyle.subheadline.monospacedDigit()).foregroundStyle(Tone.green.color)
                Text(verbatim: removed).font(TypeStyle.subheadline.monospacedDigit()).foregroundStyle(Tone.red.color)
            }
        case .entries(let label, let entries, let more):
            EntriesBlock(label: label, entries: entries, more: more)
        case .results(let results):
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                ForEach(Array(results.enumerated()), id: \.offset) { _, result in
                    VStack(alignment: .leading, spacing: Tokens.space[0]) {
                        HStack(spacing: Tokens.space[2]) {
                            Text(verbatim: result.label).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                            StatePill(word: result.word, symbol: stateSymbol(result.tone), tone: result.tone)
                        }
                        if let detail = result.detail {
                            Text(verbatim: detail).font(TypeStyle.subheadline)
                        }
                        ForEach(Array(result.notes.enumerated()), id: \.offset) { _, note in
                            Text(verbatim: note).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                        }
                    }
                    .accessibilityElement(children: .combine)
                }
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        }
    }
}

/// A labelled list of host text (titles, summaries, paths) in a flat block.
private struct EntriesBlock: View {
    let label: String
    let entries: [DetailEntry]
    let more: String?

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: "exclamationmark.triangle").imageScale(.small).accessibilityHidden(true)
                Text(verbatim: label).font(TypeStyle.footnote.weight(.semibold))
            }
            .foregroundStyle(Tokens.color(.label2))
            Divider()
            ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    if let caption = entry.caption {
                        Text(verbatim: caption).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    }
                    HStack(alignment: .firstTextBaseline, spacing: Tokens.space[2]) {
                        if let shown = entry.shown {
                            Text(verbatim: shown)
                                .font(TypeStyle.monoSmall)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } else if let line = entry.notShown {
                            Text(verbatim: line)
                                .font(TypeStyle.footnote)
                                .foregroundStyle(Tokens.color(.label2))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        if let count = entry.count {
                            Text(verbatim: count).font(TypeStyle.footnote.monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                        }
                    }
                    if let cut = entry.cutNote {
                        Text(verbatim: cut).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    }
                }
            }
            if let more {
                Text(verbatim: more).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }
}
