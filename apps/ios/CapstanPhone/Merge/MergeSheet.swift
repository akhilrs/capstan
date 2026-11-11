import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI
import UIKit

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
    /// The branches the host lets this merge go into; nil while they are read or when the host cannot list them.
    @State private var targets: MergeTargets?
    @State private var targetsNote: String?
    /// The branch chosen in the Into picker.
    @State private var into: String = ""
    @FocusState private var cancelFocused: Bool

    private var hostName: String { model.hosts.first { $0.id == row.hostId }?.displayName ?? "the host" }

    /// The set the item is a member of; the sheet then decides this repo only, in merge order (board 08).
    private var set: IntegrationSet? { PhoneSetText.set(of: item) }

    private var setSheet: SetMergeSheet? {
        set.map { SetMergeSheet.make($0, verify: item.integrationDetails?.byRepo, mainBranch: into.isEmpty ? "main" : into) }
    }

    /// What the sheet offers for a set: Merge only for the next built member; a blocked one names what to merge first.
    private var offer: PhoneSetMergeOffer? {
        setSheet.map { PhoneSetMergeOffer.make($0) { waitingKey(of: $0) } }
    }

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
                Text(verbatim: outcome == nil ? (setSheet?.title ?? "Merge") : "Merge result"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if outcome == nil {
                    ToolbarItem(placement: .cancellationAction) {
                        Button { dismiss() } label: { Text(verbatim: isBlocked ? "Close" : "Cancel") }.disabled(sending)
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { bar }
        }
        .presentationDetents([.large])
        .presentationBackground(Tokens.color(.ground))
        .interactiveDismissDisabled(sending)
        .onAppear { cancelFocused = true }
        .task { await loadTargets() }
    }

    // MARK: Confirmation

    private var confirmation: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            if let set, let sheet = setSheet { setSummary(set, sheet) }
            DetailFacts(facts: facts)
            intoPicker
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

    private var isBlocked: Bool {
        if case .blocked? = offer { return true }
        return false
    }

    /// Where this repo stands in the set: the eyebrow, its place in merge order, the repos in order with the next one
    /// tagged, the gate, the refusal when another repo comes first, and what follows.
    private func setSummary(_ set: IntegrationSet, _ sheet: SetMergeSheet) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: "repo " + set.repo + " of set " + set.setId)
                .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: sheet.order).font(TypeStyle.headline)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                ForEach(sheet.members) { member in
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: member.state.symbol).foregroundStyle(member.state.tone.color)
                            .frame(width: 20).accessibilityHidden(true)
                        Text(verbatim: member.repo)
                            .font(member.isCurrent ? TypeStyle.mono.weight(.semibold) : TypeStyle.mono)
                            .strikethrough(member.struck).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: Tokens.space[1])
                        Text(verbatim: member.state.word).font(TypeStyle.subheadline).foregroundStyle(member.state.tone.color)
                        if member.isNext { StatePill(word: SetMemberRow.nextTag, symbol: nil, tone: .accent) }
                    }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(Text(verbatim: member.voiceover))
                }
            }
            if let gate = sheet.gate {
                DetailNotice(
                    tone: gate.state == .passed ? .green : (gate.state == .failed ? .red : .yellow),
                    symbol: gate.state == .passed ? "checkmark" : "exclamationmark.triangle", title: gate.line,
                    text: gate.lead ?? "")
            }
            if let title = sheet.blockedTitle, let by = sheet.blockedBy {
                DetailNotice(
                    tone: .yellow, symbol: "exclamationmark.triangle", title: "merge " + by + " first",
                    text: title + ". " + (sheet.blockedNote ?? SetMergeSheet.blockedLead))
            } else if let note = sheet.ownNote {
                DetailNotice(tone: .yellow, symbol: "exclamationmark.triangle", title: note, text: "")
            }
            Text(verbatim: sheet.after).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
        }
    }

    /// The waiting item of another repo of this set, when the host holds one.
    private func waitingKey(of repo: String) -> String? {
        guard let set, let waiting = model.hostModels[row.hostId]?.projects[row.projectId]?.waiting.values else { return nil }
        return PhoneSetText.waitingKey(of: repo, in: set, among: Array(waiting))
    }

    /// Shows the item of another repo of the set, after the sheet closes.
    private func open(_ repo: String) {
        let key = waitingKey(of: repo)
        dismiss()
        if let key {
            model.navigation.show(.waiting, [.item(InboxRowID(hostId: row.hostId, projectId: row.projectId, key: key))])
        }
    }

    private var facts: [DetailFact] {
        var rows = [
            DetailFact(label: "Branch", value: branch, mono: true),
        ]
        if targets == nil {
            rows.append(DetailFact(label: "Into", value: "the project's branch on " + hostName))
        }
        if let head = item.headSha { rows.append(DetailFact(label: "Commit", value: String(head.prefix(12)), mono: true)) }
        return rows
    }

    /// The branch the merge goes into. The checked-out branch is merged in its checkout; any other branch moves on the
    /// host without the checkout being touched.
    @ViewBuilder
    private var intoPicker: some View {
        if let targets, !targets.branches.isEmpty {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Picker(selection: $into) {
                    ForEach(targets.branches, id: \.self) { branch in
                        Text(verbatim: label(branch, targets)).tag(branch)
                    }
                } label: {
                    Text("Into")
                }
                .pickerStyle(.navigationLink)
                .disabled(sending)
                Text(verbatim: intoNote(targets))
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(Tokens.space[3])
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        } else if let targetsNote {
            Text(verbatim: targetsNote)
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
        }
    }

    private func label(_ branch: String, _ targets: MergeTargets) -> String {
        var marks: [String] = []
        if branch == targets.checkedOut { marks.append("checked out") }
        if branch == targets.defaultBranch { marks.append("default") }
        return marks.isEmpty ? branch : branch + " (" + marks.joined(separator: ", ") + ")"
    }

    private func intoNote(_ targets: MergeTargets) -> String {
        if into == targets.checkedOut {
            return into + " is checked out on " + hostName + ": the merge updates that checkout, which must have no uncommitted changes."
        }
        return into + " is not checked out: the host moves the branch and leaves the checkout on " + hostName + " as it is."
    }

    private func loadTargets() async {
        switch await model.mergeTargets(hostId: row.hostId, projectId: row.projectId) {
        case .targets(let found) where !found.branches.isEmpty:
            targets = found
            into = found.preferred ?? found.branches[0]
        case .targets:
            targetsNote = "The host lists no branch to merge into; the merge goes into the project's branch."
        case .unsupported:
            targetsNote = nil
        case .error:
            targetsNote = "The branch list could not be read; the merge goes into the project's branch."
        }
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
        let text = MergeText.make(outcome, branch: branch, hostName: hostName, at: receivedAt, set: set)
        return VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: text.headline).font(TypeStyle.title2).fixedSize(horizontal: false, vertical: true)
            DetailNotice(
                tone: tone(text.tone), symbol: symbol(text.tone), title: text.title, text: text.line)
            if let host = text.hostText {
                DetailTextBlock(text: host, name: "From the host", shownAs: "shown as written")
            }
            if let command = text.command {
                Text(verbatim: command)
                    .font(TypeStyle.mono).textSelection(.enabled)
                    .padding(Tokens.space[2]).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            }
            if let fixLine {
                Text(verbatim: fixLine)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            } else if text.fix == .askPM {
                Text("Ask the PM sends this refusal to the project's PM, who can fix it on the host. The message also appears in Messages.")
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
                let text = MergeText.make(outcome, branch: branch, hostName: hostName, at: receivedAt, set: set)
                // A fix action has a longer label: the buttons stack, the action on top, so neither wraps.
                let hasFix = resumed || text.fix == .resumeProject || (text.fix == .askPM && fixLine == nil)
                let stack = typeSize.isAccessibilitySize || hasFix
                    ? AnyLayout(VStackLayout(spacing: Tokens.space[2])) : AnyLayout(HStackLayout(spacing: Tokens.space[2]))
                stack {
                    if let pending {
                        Button { retry(pending) } label: {
                            if sending { SendingLabel() } else { Text("Retry") }
                        }
                        .buttonStyle(.capstanQuiet)
                        .disabled(sending)
                    }
                    if let repo = text.openRepo {
                        Button("Close") { dismiss() }.buttonStyle(.capstanQuiet)
                        Button { open(repo) } label: { Text(verbatim: "Open " + repo) }.buttonStyle(.capstanPrimary)
                    } else if let command = text.command {
                        Button("Close") { dismiss() }.buttonStyle(.capstanQuiet)
                        Button { UIPasteboard.general.string = command } label: { Text("Copy command") }
                            .buttonStyle(.capstanPrimary)
                    } else if let next = text.nextRepo {
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                        // Merge <next>… is offered only when the next repo has an item to open.
                        if waitingKey(of: next) != nil {
                            Button { open(next) } label: { Text(verbatim: "Merge " + next + "…") }.buttonStyle(.capstanPrimary)
                        }
                    } else if resumed {
                        Button { mergeAgain() } label: {
                            if sending { SendingLabel() } else { Text("Merge again") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(sending)
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    } else if text.fix == .resumeProject {
                        Button { resumeProject() } label: {
                            if fixing { SendingLabel() } else { Text("Resume project") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(fixing)
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    } else if text.fix == .askPM && fixLine == nil {
                        Button { askPM(text) } label: {
                            if fixing { SendingLabel() } else { Text("Ask the PM") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(fixing)
                        Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    } else {
                        Button("Done") { dismiss() }.buttonStyle(.capstanPrimary)
                    }
                }
            } else {
                let stack = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(spacing: Tokens.space[2])) : AnyLayout(HStackLayout(spacing: Tokens.space[2]))
                stack {
                    Button { dismiss() } label: { Text(verbatim: isBlocked ? "Close" : "Cancel") }
                        .buttonStyle(.capstanQuiet)
                        .keyboardShortcut(.cancelAction)
                        .focused($cancelFocused)
                        .disabled(sending)
                    switch offer {
                    case .some(.blocked(let repo?)):
                        // Open only where the blocking repo has a waiting item to show.
                        Button { open(repo) } label: { Text(verbatim: "Open " + repo) }.buttonStyle(.capstanPrimary)
                    case .some(.blocked(nil)):
                        EmptyView()
                    default:
                        Button { merge() } label: {
                            if sending { SendingLabel() } else { Text(verbatim: setSheet?.action ?? "Merge") }
                        }
                        .buttonStyle(.capstanPrimary)
                        .disabled(sending || offer == .nothingToMerge)
                    }
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
            let verb: DecideVerb = targets != nil && !into.isEmpty ? .integrationMergeInto(into) : .integrationMerge
            let result = await model.decide(verb, on: row)
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
