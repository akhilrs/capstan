import AppKit
import CapstanAppCore
import CapstanKit
import SwiftUI

/// The body of an integration item in the inspector: the state, what the integration is for, its impact and results, and
/// the next steps. Host text is plain text in flat blocks; nothing here is a link.
struct IntegrationDetailsView: View {
    let rows: IntegrationDetailRows
    let onMerge: (() -> Void)?

    @State private var expanded: Set<String>

    init(rows: IntegrationDetailRows, startFolded: Bool = false, onMerge: (() -> Void)? = nil) {
        self.rows = rows
        self.onMerge = onMerge
        _expanded = State(initialValue: startFolded ? [] : Set(rows.sections.map { $0.title }))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            StatePill(word: rows.stateWord, symbol: symbol, tone: rows.stateTone)
            if let lead = rows.lead {
                Text(verbatim: lead).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            }
            MetaList(rows: rows.meta.map { MetaRow(label: $0.label, value: $0.value) })
            ForEach(Array(rows.banners.enumerated()), id: \.offset) { _, block in
                blockView(block)
            }
            ForEach(rows.sections, id: \.title) { section in
                sectionView(section)
            }
            if !rows.steps.isEmpty {
                stepsView
            }
        }
    }

    private var symbol: String {
        switch rows.stateTone {
        case .yellow: return "exclamationmark.triangle.fill"
        case .green: return "checkmark.circle.fill"
        default: return "clock.fill"
        }
    }

    // MARK: Sections

    private func sectionView(_ section: DetailSection) -> some View {
        let isOpen = Binding<Bool>(
            get: { expanded.contains(section.title) },
            set: { open in
                if open { expanded.insert(section.title) } else { expanded.remove(section.title) }
            })
        return DisclosureGroup(isExpanded: isOpen) {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                ForEach(Array(section.blocks.enumerated()), id: \.offset) { _, block in
                    blockView(block)
                }
            }
            .padding(.top, Tokens.space[1])
        } label: {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: section.title).font(.system(size: 13, weight: .semibold))
                if let summary = section.summary {
                    Text(verbatim: summary).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                }
            }
        }
    }

    @ViewBuilder
    private func blockView(_ block: DetailBlock) -> some View {
        switch block {
        case .note(let text):
            Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        case .banner(let tone, let title, let text):
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Banner(tone: tone, text: title)
                Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        case .stat(let files, let added, let removed):
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: files).font(.system(size: 15, weight: .semibold)).monospacedDigit()
                Text(verbatim: added).font(.system(size: 13)).foregroundStyle(Tone.green.color).monospacedDigit()
                Text(verbatim: removed).font(.system(size: 13)).foregroundStyle(Tone.red.color).monospacedDigit()
            }
        case .entries(let label, let entries, let more):
            entriesView(label: label, entries: entries, more: more)
        case .results(let results):
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                ForEach(Array(results.enumerated()), id: \.offset) { _, result in
                    resultView(result)
                }
            }
        }
    }

    private func entriesView(label: String, entries: [DetailEntry], more: String?) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[0]) {
                Image(systemName: "exclamationmark.triangle").imageScale(.small)
                Text(verbatim: label).font(.system(size: 12, weight: .semibold))
            }
            .foregroundStyle(Tokens.color(.label2))
            Divider()
            ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                entryView(entry)
            }
            if let more {
                Text(verbatim: more).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }

    private func entryView(_ entry: DetailEntry) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            if let caption = entry.caption {
                Text(verbatim: caption).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                if let shown = entry.shown {
                    Text(verbatim: shown)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Tokens.color(.label))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else if let line = entry.notShown {
                    Text(verbatim: line)
                        .font(.system(size: 12))
                        .foregroundStyle(Tokens.color(.label2))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if let count = entry.count {
                    Text(verbatim: count).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).monospacedDigit()
                }
            }
            if let cut = entry.cutNote {
                Text(verbatim: cut).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    private func resultView(_ result: DetailResult) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: result.label).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                StatePill(word: result.word, symbol: nil, tone: result.tone)
                if let detail = result.detail {
                    Text(verbatim: detail).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                }
            }
            ForEach(Array(result.notes.enumerated()), id: \.offset) { _, note in
                Text(verbatim: note).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    // MARK: Next steps

    private var stepsView: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: "Next steps").font(.system(size: 13, weight: .semibold))
            ForEach(Array(rows.steps.enumerated()), id: \.offset) { index, step in
                HStack(alignment: .top, spacing: Tokens.space[1]) {
                    Text(verbatim: String(index + 1))
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(index == 0 ? Tokens.color(.accent) : Tokens.color(.label2))
                        .frame(width: 16, alignment: .trailing)
                    stepBody(step)
                }
            }
        }
    }

    private func stepBody(_ step: DetailStep) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if let title = step.title {
                Text(verbatim: title).font(.system(size: 13, weight: .semibold))
            }
            if let note = step.note {
                Text(verbatim: note).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            if let command = step.command {
                Text(verbatim: command)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(Tokens.space[1])
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
                Button { copy(command) } label: { Text("Copy commands") }.buttonStyle(.capstanPrimary)
            }
            if step.action == .merge, let onMerge {
                Button { onMerge() } label: { Text("Merge…") }.buttonStyle(.capstanPrimary)
            }
            if let host = step.hostText {
                UntrustedTextBlock(text: host.text, label: "From the host · untrusted text")
            }
        }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

#Preview("IntegrationDetailsView") {
    let text = UntrustedText(raw: "Search for the docs site", verified: true, truncated: false)
    let details = IntegrationDetails(
        recorded: true, plans: [IntegrationPlan(planId: "plan-12", title: text)],
        impact: IntegrationImpact(filesChanged: 23, insertions: 1284, deletions: 96),
        review: IntegrationReview(verdict: "passed", reviewerAgentId: "reviewer-1", round: 1))
    let item = StoredWaiting(
        key: "integration:7", requestId: "integration:int-7", kind: .integration, since: "2026-10-29T08:00:00Z",
        summary: UntrustedText(raw: "merged integration/plan-12", verified: true, truncated: false),
        contentHash: "aaaaaaaaaaaa", needsTerminal: false, integrationState: "merged",
        branch: UntrustedText(raw: "integration/plan-12", verified: true, truncated: false),
        headSha: "7c41e9a02d5f", integrationDetails: details)
    ScrollView {
        IntegrationDetailsView(rows: IntegrationDetailRows.make(item, hostName: "build-host", now: Date()))
            .padding()
    }
    .frame(width: 360)
}
