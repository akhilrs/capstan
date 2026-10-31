import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Merge… for a branch ready to merge (board 10), at the large detent: the branch, the commit, review and verify as
/// the host last sent them, then Merge, with Cancel as the default. The host checks everything again just before it
/// merges, and never sends it to a remote (D8): there is no push switch. The result replaces the confirmation in the same sheet.
struct MergeSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize
    let row: InboxRowID
    let item: StoredWaiting

    @State private var sending = false
    @State private var outcome: DecideOutcome?
    @State private var receivedAt = Date()
    /// A fix the person started from a refusal (resume the project, ask the PM) is on its way.
    @State private var fixing = false
    /// What the fix did, in a line; nil before one ran.
    @State private var fixLine: String?
    /// The project was resumed from this sheet: the bar offers the merge again.
    @State private var resumed = false
    @FocusState private var cancelFocused: Bool

    private var hostName: String { model.hosts.first { $0.id == row.hostId }?.displayName ?? "the host" }

    private var branch: String {
        guard let branch = item.branch, branch.verified else { return "the integration branch" }
        return TextBudget.displayPrefix(branch.displayPlain, maxBytes: 128).text
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[3]) {
                    if let outcome {
                        result(outcome)
                    } else {
                        confirmation
                    }
                }
                .padding(Tokens.margin)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Tokens.color(.ground))
            .navigationTitle(
                Text(verbatim: outcome == nil ? "Merge" : "Merge result"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if outcome == nil {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }.disabled(sending)
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { bar }
        }
        .presentationDetents([.large])
        .presentationBackground(Tokens.color(.ground))
        .interactiveDismissDisabled(sending)
        .onAppear { cancelFocused = true }
    }

    // MARK: Confirmation

    private var confirmation: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            DetailFacts(facts: facts)
            HStack(spacing: Tokens.space[2]) {
                review
                verify
            }
            Text("The host checks the branch, the commit, the review and the verify again just before it merges. A refused or conflicting merge changes nothing in the checkout. The merge stays on the host: Capstan sends it to no remote.")
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var facts: [DetailFact] {
        var rows = [
            DetailFact(label: "Branch", value: branch, mono: true),
            DetailFact(label: "Into", value: "the project's branch on " + hostName),
        ]
        if let head = item.headSha { rows.append(DetailFact(label: "Commit", value: String(head.prefix(12)), mono: true)) }
        return rows
    }

    @ViewBuilder
    private var review: some View {
        switch item.integrationDetails?.review?.verdict {
        case "passed"?: StatePill(word: "Review passed", symbol: "checkmark", tone: .green)
        case let verdict?: StatePill(word: "Review " + verdict, symbol: "exclamationmark.triangle", tone: .yellow)
        case nil: StatePill(word: "Review not known", symbol: nil, tone: .grey)
        }
    }

    @ViewBuilder
    private var verify: some View {
        if let latest = item.integrationDetails?.verify?.latest, latest.commitSha == item.headSha {
            StatePill(
                word: "Verify " + latest.state, symbol: latest.state == "passed" ? "checkmark" : "exclamationmark.triangle",
                tone: latest.state == "passed" ? .green : .yellow)
        } else {
            StatePill(word: "Verify not known", symbol: nil, tone: .grey)
        }
    }

    // MARK: Result

    private func result(_ outcome: DecideOutcome) -> some View {
        let text = MergeText.make(outcome, branch: branch, hostName: hostName, at: receivedAt)
        return VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: text.headline).font(TypeStyle.title2).fixedSize(horizontal: false, vertical: true)
            DetailNotice(
                tone: tone(text.tone), symbol: symbol(text.tone), title: text.title, text: text.line)
            if let host = text.hostText {
                DetailTextBlock(text: host, name: "From the host", shownAs: "shown as written")
            }
            if let fixLine {
                Text(verbatim: fixLine)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            } else if text.fix == .askPM {
                Text("Ask the PM to fix this sends the refusal to the project's PM, who can fix the checkout on the host. The message also appears in Messages.")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func tone(_ tone: MergeText.Tone) -> Tone {
        switch tone {
        case .good: return .green
        case .warning: return .yellow
        case .bad: return .red
        }
    }

    private func symbol(_ tone: MergeText.Tone) -> String {
        switch tone {
        case .good: return "checkmark"
        case .warning: return "exclamationmark.triangle"
        case .bad: return "xmark"
        }
    }

    // MARK: Bar

    @ViewBuilder
    private var bar: some View {
        Group {
            if let outcome {
                let pending = OutcomeLines.pendingId(outcome)
                let stack = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(spacing: Tokens.space[2])) : AnyLayout(HStackLayout(spacing: Tokens.space[2]))
                let text = MergeText.make(outcome, branch: branch, hostName: hostName, at: receivedAt)
                stack {
                    if let pending {
                        Button { retry(pending) } label: {
                            if sending { SendingLabel() } else { Text("Retry") }
                        }
                        .buttonStyle(.capstanQuiet)
                        .disabled(sending)
                    }
                    if resumed {
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                        Button { mergeAgain() } label: {
                            if sending { SendingLabel() } else { Text("Merge again") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(sending)
                    } else if text.fix == .resumeProject {
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                        Button { resumeProject() } label: {
                            if fixing { SendingLabel() } else { Text("Resume project") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(fixing)
                    } else if text.fix == .askPM && fixLine == nil {
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                        Button { askPM(text) } label: {
                            if fixing { SendingLabel() } else { Text("Ask the PM to fix this") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(fixing)
                    } else {
                        Button("Done") { dismiss() }.buttonStyle(.capstanPrimary)
                    }
                }
            } else {
                let stack = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(spacing: Tokens.space[2])) : AnyLayout(HStackLayout(spacing: Tokens.space[2]))
                stack {
                    Button("Cancel") { dismiss() }
                        .buttonStyle(.capstanQuiet)
                        .keyboardShortcut(.cancelAction)
                        .focused($cancelFocused)
                        .disabled(sending)
                    Button { merge() } label: {
                        if sending { SendingLabel() } else { Text("Merge") }
                    }
                    .buttonStyle(.capstanPrimary)
                    .disabled(sending)
                }
            }
        }
        .padding(.horizontal, Tokens.margin)
        .padding(.vertical, Tokens.space[2])
        .background(Tokens.color(.ground))
    }

    // MARK: Actions

    private func merge() {
        guard !sending else { return }
        sending = true
        Task {
            let result = await model.decide(.integrationMerge, on: row)
            finish(result)
        }
    }

    private func mergeAgain() {
        outcome = nil
        fixLine = nil
        resumed = false
        merge()
    }

    private func resumeProject() {
        guard !fixing else { return }
        fixing = true
        Task {
            let result = await model.decideRun(.runResume(reason: nil), hostId: row.hostId, projectId: row.projectId)
            fixing = false
            switch result {
            case .applied, .forwarded:
                Haptics.play(.success)
                resumed = true
                fixLine = "The project is resumed. Merge again when you are ready."
            default:
                Haptics.play(.warning)
                fixLine = "The project could not be resumed from here. Resume it from the project screen or with cstan resume on " + hostName + "."
            }
        }
    }

    /// Sends the refusal to the project's PM as a message from the phone. The host's reason goes as text; the PM is
    /// told it came from the user's remote client.
    private func askPM(_ text: MergeText) {
        guard !fixing else { return }
        fixing = true
        let hostReason = text.hostText.map { TextBudget.displayPrefix($0.displayPlain, maxBytes: 240).text } ?? ""
        let message = "Please fix this so the integration can be merged: merging " + branch + " from my phone was refused: "
            + text.reason + "." + (hostReason.isEmpty ? "" : " The host said: " + hostReason)
            + " Tell me when it is ready and I will merge it again."
        Task {
            let result = await model.messagePM(
                String(message.prefix(SentMessageLog.textCap)), hostId: row.hostId, projectId: row.projectId)
            fixing = false
            switch result {
            case .applied, .forwarded:
                Haptics.play(.success)
                fixLine = "Sent to the PM. Its answer arrives in Messages; merge again when it says the checkout is ready."
            default:
                Haptics.play(.warning)
                fixLine = "The message did not reach the PM. It is kept in Messages, where you can send it again."
            }
        }
    }

    private func retry(_ id: UUID) {
        guard !sending else { return }
        sending = true
        Task {
            let result = await model.retry(id)
            finish(result)
        }
    }

    private func finish(_ result: DecideOutcome) {
        receivedAt = Date()
        outcome = result
        sending = false
        switch result {
        case .applied, .forwarded: Haptics.play(.success)
        case .refused: Haptics.play(.warning)
        case .notSent, .malformed: Haptics.play(.error)
        }
    }
}
