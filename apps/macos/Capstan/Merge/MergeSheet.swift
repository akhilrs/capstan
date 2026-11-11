import AppKit
import CapstanAppCore
import CapstanKit
import SwiftUI

/// Merge… for a branch ready to merge (design-actions boards 1 and 2): the branch, the commit, the Into picker, review
/// and verify as the host last sent them, then Merge, with Cancel as the default. The host checks everything again just
/// before it merges and never sends it to a remote: there is no push control. The result replaces the confirmation in
/// the same sheet, and a merge is never retried by itself.
struct MergeSheet: View {
    let model: AppModel
    let row: InboxRowID
    let item: StoredWaiting

    @Environment(\.dismiss) private var dismiss
    @State private var sending = false
    @State private var outcome: DecideOutcome?
    @State private var receivedAt = Date()
    /// A fix started from a refusal (resume the project, ask the PM) is on its way.
    @State private var fixing = false
    @State private var fixLine: String?
    /// The project was resumed from this sheet: the bar offers the merge again.
    @State private var resumed = false
    /// The branches the host lets this merge go into; nil while they are read or when the host cannot list them.
    @State private var targets: MergeTargets?
    @State private var targetsNote: String?
    @State private var into = ""
    @FocusState private var cancelFocused: Bool

    private var hostName: String { model.hosts.first { $0.id == row.hostId }?.displayName ?? "the host" }

    /// The set the item is a member of; the sheet then decides this repo only, in merge order.
    private var set: IntegrationSet? { item.set ?? item.integrationDetails?.set }

    private var setSheet: SetMergeSheet? {
        set.map { SetMergeSheet.make($0, verify: item.integrationDetails?.byRepo, mainBranch: into.isEmpty ? "main" : into) }
    }

    private var branch: String {
        guard let branch = item.branch, branch.verified else { return "the integration branch" }
        return TextBudget.displayPrefix(branch.displayPlain, maxBytes: 128).text
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: outcome == nil ? (setSheet?.title ?? "Merge " + branch) : "Merge result")
                .font(.system(size: 15, weight: .semibold))
            if let outcome {
                result(outcome)
            } else {
                confirmation
            }
            bar
        }
        .padding(Tokens.space[4])
        .frame(width: 460)
        .interactiveDismissDisabled(sending)
        .onAppear { cancelFocused = true }
        .task { await loadTargets() }
    }

    // MARK: Confirmation

    private var confirmation: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if let setSheet { setSummary(setSheet) }
            MetaList(rows: facts)
            intoPicker
            HStack(spacing: Tokens.space[1]) {
                review
                verify
            }
            Text(
                "The host checks the branch, the commit, the review and the verify again just before it merges. A refused or conflicting merge changes nothing in the checkout. The merge stays on the host: Capstan sends it to no remote."
            )
            .font(.system(size: 12))
            .foregroundStyle(Tokens.color(.label2))
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// Where this repo stands in the set: its place in merge order, the repos in order, the gate, and what follows.
    private func setSummary(_ sheet: SetMergeSheet) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: sheet.order).font(.system(size: 13, weight: .semibold))
            VStack(alignment: .leading, spacing: 2) {
                ForEach(sheet.members) { member in
                    HStack(spacing: Tokens.space[0]) {
                        Image(systemName: member.state.symbol).foregroundStyle(member.state.tone.color)
                            .frame(width: 16).accessibilityHidden(true)
                        Text(verbatim: member.repo)
                            .font(.system(size: 12, weight: member.isCurrent ? .semibold : .regular, design: .monospaced))
                            .strikethrough(member.struck).lineLimit(1).truncationMode(.middle)
                        Text(verbatim: member.state.word).font(.system(size: 12)).foregroundStyle(member.state.tone.color)
                        if member.isNext { StatePill(word: SetMemberRow.nextTag, symbol: nil, tone: .accent) }
                    }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(member.voiceover)
                }
            }
            if let gate = sheet.gate {
                Banner(tone: gate.state == .passed ? .green : (gate.state == .failed ? .red : .yellow), text: gate.line)
            }
            if let title = sheet.blockedTitle, let by = sheet.blockedBy {
                Banner(tone: .yellow, text: "merge " + by + " first")
                Text(verbatim: title + ". " + (sheet.blockedNote ?? SetMergeSheet.blockedLead))
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            } else if let note = sheet.ownNote {
                Banner(tone: .yellow, text: note)
            }
            Text(verbatim: sheet.after).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        }
    }

    private func plain(_ text: String) -> UntrustedText { UntrustedText(raw: text, verified: true, truncated: false) }

    private var facts: [MetaRow] {
        var rows = [MetaRow(label: "Branch", value: plain(branch))]
        if targets == nil { rows.append(MetaRow(label: "Into", value: plain("the project's branch on " + hostName))) }
        if let head = item.headSha { rows.append(MetaRow(label: "Commit", value: plain(String(head.prefix(12))))) }
        return rows
    }

    /// The checked-out branch is merged in its checkout; any other branch moves on the host with the checkout untouched.
    @ViewBuilder
    private var intoPicker: some View {
        if let targets, !targets.branches.isEmpty {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Picker("Into", selection: $into) {
                    ForEach(targets.branches, id: \.self) { branch in
                        Text(verbatim: label(branch, targets)).tag(branch)
                    }
                }
                .disabled(sending)
                Text(verbatim: intoNote(targets))
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else if let targetsNote {
            Text(verbatim: targetsNote).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
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
            return into + " is checked out on " + hostName
                + ": the merge updates that checkout, which must have no uncommitted changes."
        }
        return into + " is not checked out: the host moves the branch and leaves the checkout on " + hostName
            + " as it is."
    }

    private func loadTargets() async {
        switch await model.mergeTargets(hostId: row.hostId, projectId: row.projectId) {
        case .targets(let found) where !found.branches.isEmpty:
            targets = found
            into = found.preferred ?? found.branches[0]
        case .targets:
            targetsNote = "The host lists no branch to merge into; the merge goes into the project's branch."
        case .unsupported:
            targetsNote = "This host cannot list branches; the merge goes into the project's default branch."
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

    private func text(_ outcome: DecideOutcome) -> MergeText {
        MergeText.make(outcome, branch: branch, hostName: hostName, at: receivedAt, set: set)
    }

    private func result(_ outcome: DecideOutcome) -> some View {
        let made = text(outcome)
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: made.headline).font(.system(size: 20, weight: .semibold))
            Banner(tone: tone(made.tone), text: made.title)
            Text(verbatim: made.line)
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
            if let host = made.hostText { UntrustedTextBlock(text: host, label: "From the host · untrusted text") }
            if let command = made.command {
                Text(verbatim: command)
                    .font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                    .padding(Tokens.space[1]).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
            }
            if let fixLine {
                Text(verbatim: fixLine).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            } else if made.fix == .askPM {
                Text("Ask the PM to fix this sends the refusal and its reason to the project's PM. The message also appears in Messages.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
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

    private func pendingId(_ outcome: DecideOutcome) -> UUID? {
        switch outcome {
        case .notSent(_, let id): return id
        case .malformed(_, let id): return id
        default: return nil
        }
    }

    // MARK: Bar

    @ViewBuilder
    private var bar: some View {
        HStack(spacing: Tokens.space[1]) {
            Spacer()
            if let outcome {
                let made = text(outcome)
                if let pending = pendingId(outcome) {
                    Button { retry(pending) } label: { Text(verbatim: sending ? "Sending…" : "Retry") }
                        .buttonStyle(.capstanQuiet).disabled(sending)
                }
                if let repo = made.openRepo {
                    Button("Close") { dismiss() }.buttonStyle(.capstanQuiet)
                    Button { open(repo) } label: { Text(verbatim: "Open " + repo) }
                        .buttonStyle(.capstanPrimary).keyboardShortcut(.defaultAction)
                } else if let command = made.command {
                    Button("Close") { dismiss() }.buttonStyle(.capstanQuiet)
                    Button { copy(command) } label: { Text("Copy command") }.buttonStyle(.capstanPrimary)
                } else if let next = made.nextRepo {
                    Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    Button { open(next) } label: { Text(verbatim: "Merge " + next + "…") }
                        .buttonStyle(.capstanPrimary).keyboardShortcut(.defaultAction)
                } else if resumed {
                    Button("Done") { dismiss() }.buttonStyle(.capstanQuiet).disabled(sending)
                    Button { mergeAgain() } label: { Text(verbatim: sending ? "Sending…" : "Merge again") }
                        .buttonStyle(.capstanPrimary).disabled(sending)
                } else if made.fix == .resumeProject {
                    Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    Button { resumeProject() } label: { Text(verbatim: fixing ? "Sending…" : "Resume project") }
                        .buttonStyle(.capstanPrimary).disabled(fixing)
                } else if made.fix == .askPM && fixLine == nil {
                    Button("Done") { dismiss() }.buttonStyle(.capstanQuiet)
                    Button { askPM(made) } label: { Text(verbatim: fixing ? "Sending…" : "Ask the PM to fix this") }
                        .buttonStyle(.capstanPrimary).disabled(fixing)
                } else {
                    Button("Done") { dismiss() }.buttonStyle(.capstanPrimary).keyboardShortcut(.defaultAction)
                }
            } else {
                Button(setSheet?.blockedBy != nil ? "Close" : "Cancel") { dismiss() }
                    .buttonStyle(.capstanQuiet)
                    .keyboardShortcut(.cancelAction)
                    .focused($cancelFocused)
                    .disabled(sending)
                if let sheet = setSheet, let by = sheet.blockedBy {
                    // Open only where the repo has a waiting item to show; else the note stands with Close alone.
                    if waitingKey(of: by) != nil {
                        Button { open(by) } label: { Text(verbatim: "Open " + by) }.buttonStyle(.capstanPrimary)
                    }
                } else {
                    Button { merge() } label: { Text(verbatim: sending ? "Sending…" : (setSheet?.action ?? "Merge")) }
                        .buttonStyle(.capstanPrimary)
                        .disabled(sending || !(setSheet?.canMerge ?? true))
                }
            }
        }
    }

    // MARK: Actions

    private func merge() {
        guard !sending else { return }
        sending = true
        let branch: String? = targets != nil && !into.isEmpty ? into : nil
        Task { finish(await model.merge(row, into: branch)) }
    }

    /// Shows the item of another repo of the set in the inspector: the one the host said has to be merged first, or the
    /// one that follows. The members other than the next one are not rows of the list; the inspector reads any item.
    private func waitingKey(of repo: String) -> String? {
        let waiting = model.hostModels[row.hostId]?.projects[row.projectId]?.waiting.values
        return waiting?.first(where: { $0.set?.repo == repo && $0.set?.setId == set?.setId })?.key
    }

    private func open(_ repo: String) {
        if let key = waitingKey(of: repo) {
            model.selectedRow = InboxRowID(hostId: row.hostId, projectId: row.projectId, key: key)
        }
        dismiss()
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
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
                resumed = true
                fixLine = "The project is resumed. Merge again when you are ready."
            default:
                fixLine = "The project could not be resumed from here. Resume it with cstan resume on " + hostName + "."
            }
        }
    }

    /// Sends the refusal and its reason to the project's PM; the host's text goes as text, cut to fit.
    private func askPM(_ made: MergeText) {
        guard !fixing else { return }
        fixing = true
        let hostReason = made.hostText.map { TextBudget.displayPrefix($0.displayPlain, maxBytes: 240).text } ?? ""
        let message = "Please fix this so the integration can be merged: merging " + branch + " from the Mac app was refused: "
            + made.reason + "." + (hostReason.isEmpty ? "" : " The host said: " + hostReason)
            + " Tell me when it is ready and I will merge it again."
        Task {
            let result = await model.messagePM(
                String(message.prefix(SentMessageLog.textCap)), hostId: row.hostId, projectId: row.projectId)
            fixing = false
            switch result {
            case .applied, .forwarded:
                fixLine = "Sent to the PM. Its answer arrives in Messages; merge again when it says the checkout is ready."
            default:
                fixLine = "The message did not reach the PM. It is kept in Messages, where you can send it again."
            }
        }
    }

    private func retry(_ id: UUID) {
        guard !sending else { return }
        sending = true
        Task { finish(await model.retry(id)) }
    }

    private func finish(_ result: DecideOutcome) {
        receivedAt = Date()
        outcome = result
        sending = false
    }
}
