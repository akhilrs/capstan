import CapstanAppCore
import CapstanKit
import SwiftUI

/// Recent outcomes of the project: the lines of the history and its state in words. Each line is stable by its id,
/// so a new outcome does not move the line the person selected.
struct RecentOutcomesView: View {
    let content: OverviewModel
    let selectedOutcome: String?
    var onSelect: (String) -> Void = { _ in }
    var onRetry: () -> Void = {}

    @State private var showAll = false

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if let gap = content.historyGapText {
                note(gap, symbol: "clock.badge.exclamationmark", tone: .yellow)
            }
            stateLine
            if let empty = content.outcomesEmptyText {
                Text(verbatim: empty)
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
            }
            ForEach(shownOutcomes) { outcome in
                line(outcome)
            }
            if content.outcomes.count > OverviewModel.collapsedOutcomeCount {
                Button { showAll.toggle() } label: {
                    Text(verbatim: showAll ? "Show fewer" : "Show \(content.outcomes.count - OverviewModel.collapsedOutcomeCount) more")
                }
                .buttonStyle(.capstanQuiet)
            }
        }
    }

    private var shownOutcomes: [OverviewOutcome] {
        showAll ? content.outcomes : Array(content.outcomes.prefix(OverviewModel.collapsedOutcomeCount))
    }

    // MARK: History state

    @ViewBuilder
    private var stateLine: some View {
        if let text = content.historyText {
            switch content.historyState {
            case .loading:
                HStack(spacing: Tokens.space[1]) {
                    ProgressView().controlSize(.small)
                    Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                }
                .accessibilityElement(children: .combine)
            case .error:
                HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                    note(text, symbol: "exclamationmark.circle", tone: .red)
                    Spacer(minLength: Tokens.space[0])
                    Button(action: onRetry) { Text("Retry") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(!content.canRetryHistory)
                }
            case .sessionOnly, .truncated:
                note(text, symbol: "info.circle", tone: .grey)
            case .idle, .backfilled:
                Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    private func note(_ text: String, symbol: String, tone: Tone) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[0] + 2) {
            Image(systemName: symbol).imageScale(.small).foregroundStyle(tone.color)
            Text(verbatim: text)
                .font(.system(size: 12))
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: Lines

    private func line(_ outcome: OverviewOutcome) -> some View {
        let selected = selectedOutcome == outcome.id
        return Button { onSelect(outcome.id) } label: {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                    icon(outcome, selected: selected)
                    text(outcome)
                    Spacer(minLength: Tokens.space[0])
                    time(outcome)
                }
                HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                    icon(outcome, selected: selected)
                    VStack(alignment: .leading, spacing: 2) {
                        text(outcome)
                        time(outcome)
                    }
                    Spacer(minLength: 0)
                }
            }
            .padding(.vertical, Tokens.space[0])
            .padding(.horizontal, Tokens.space[1])
            .background(
                RoundedRectangle(cornerRadius: Tokens.radius.control)
                    .fill(selected ? Tokens.color(.accent).opacity(0.16) : Color.clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func icon(_ outcome: OverviewOutcome, selected: Bool) -> some View {
        Image(systemName: outcome.symbol)
            .frame(width: 18)
            .foregroundStyle(selected ? Tokens.color(.accent) : Tokens.color(.label2))
    }

    private func text(_ outcome: OverviewOutcome) -> some View {
        Text(verbatim: outcome.text)
            .font(.system(size: 13))
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func time(_ outcome: OverviewOutcome) -> some View {
        Text(verbatim: outcome.hostTime)
            .font(.system(size: 12).monospacedDigit())
            .foregroundStyle(Tokens.color(.label2))
            .fixedSize()
    }
}

#if DEBUG
private struct HistoryPreview: View {
    let kind: OverviewSamples.HistoryKind

    var body: some View {
        let content = OverviewSamples.content(history: kind)
        VStack(alignment: .leading) {
            RecentOutcomesView(content: content, selectedOutcome: nil)
            Spacer(minLength: 0)
        }
        .padding()
        .frame(width: 420, height: 360)
    }
}

#Preview("History20Loading") { HistoryPreview(kind: .loading) }
#Preview("History20Backfilled") { HistoryPreview(kind: .backfilled) }
#Preview("History20SessionOnly") { HistoryPreview(kind: .sessionOnly) }
#Preview("History20Error") { HistoryPreview(kind: .error) }
#Preview("History20Truncated") { HistoryPreview(kind: .truncated) }
#Preview("History20Gap") { HistoryPreview(kind: .gap) }
#endif
