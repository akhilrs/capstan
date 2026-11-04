import CapstanAppCore
import CapstanKit
import SwiftUI

/// The inspector body of an integration item: its state, impact, results and next steps, with Merge… where the host can
/// merge and Discard integration… behind a destructive confirmation. Neither runs without the person confirming it.
struct IntegrationInspector: View {
    let model: AppModel
    let row: InboxRowID
    let item: StoredWaiting
    let hostName: String

    @State private var merging = false
    @State private var confirmDiscard = false
    @State private var discarding = false
    @State private var discardOutcome: DecideOutcome?
    @State private var into: String?

    private var canMerge: Bool { model.availability(.integrationMerge, row: row, option: nil) == .available }
    private var canDiscard: Bool { model.availability(.integrationDiscard, row: row, option: nil) == .available }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            IntegrationDetailsView(
                rows: IntegrationDetailRows.make(
                    item, hostName: hostName, now: Date(), mergeAvailable: canMerge, into: into),
                onMerge: canMerge ? { merging = true } : nil)
            if canDiscard {
                Button(role: .destructive) { confirmDiscard = true } label: {
                    Text(verbatim: discarding ? "Discarding…" : "Discard integration…")
                }
                .buttonStyle(.capstanDestructive)
                .disabled(discarding)
            }
            if let discardOutcome { discardResult(discardOutcome) }
        }
        .task(id: row) {
            if canMerge, case .targets(let found) = await model.mergeTargets(hostId: row.hostId, projectId: row.projectId) {
                into = found.preferred
            }
        }
        .sheet(isPresented: $merging) { MergeSheet(model: model, row: row, item: item) }
        .sheet(isPresented: $confirmDiscard) {
            ConfirmSheet(
                title: "Discard this integration?", detail: item.branch ?? item.summary, noteLimit: nil,
                destructiveLabel: "Discard", onConfirm: { _ in discard() })
        }
    }

    @ViewBuilder
    private func discardResult(_ outcome: DecideOutcome) -> some View {
        switch outcome {
        case .applied, .forwarded:
            Banner(tone: .green, text: "Discarded. The host removed the branch.")
        case .refused:
            Banner(tone: .yellow, text: "The host refused to discard it. Nothing changed.")
        case .notSent, .malformed:
            Banner(tone: .red, text: "Not confirmed. Read the item again before trying once more.")
        }
    }

    private func discard() {
        guard !discarding else { return }
        discarding = true
        discardOutcome = nil
        Task {
            discardOutcome = await model.discardIntegration(row)
            discarding = false
        }
    }
}
