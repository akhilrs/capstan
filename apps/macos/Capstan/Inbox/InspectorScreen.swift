import AppKit
import CapstanAppCore
import CapstanKit
import SwiftUI

/// The inspector of the selected inbox row: what the host sent, and the decisions the person can take.
struct InspectorScreen: View {
    let model: AppModel
    let row: InboxRowID?

    @State private var outcome: DecideOutcome?
    @State private var sending = false
    @State private var lastVerb: DecideVerb?
    @State private var showOutcome = false
    @State private var confirmDeny = false
    @State private var linkToConfirm: UntrustedText?

    var body: some View {
        Group {
            if let row, let item = model.item(row) {
                content(row, item)
            } else {
                noSelection
            }
        }
        .onChange(of: row) { _, _ in
            outcome = nil
            sending = false
            showOutcome = false
            lastVerb = nil
        }
    }

    /// No row is selected: the outcome line the person chose, else the summary of the selected project, else a hint.
    @ViewBuilder
    private var noSelection: some View {
        if case .project(let hostId, let projectId) = model.selection,
            let content = model.overviewContent(hostId: hostId, projectId: projectId),
            let project = model.project(hostId, projectId)
        {
            let name = HostLabel.projectName(project)
            let host = hostName(hostId)
            if let id = model.overview.selectedOutcomeId, let outcome = content.outcomes.first(where: { $0.id == id }) {
                OutcomeInspectorView(outcome: outcome, projectName: name, hostName: host)
            } else {
                ProjectSummaryView(content: content, projectName: name, hostName: host)
            }
        } else {
            Text("Select an item to see what it asks")
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.label2))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func content(_ row: InboxRowID, _ item: StoredWaiting) -> some View {
        // A project that is not ready shows its last rows with the decisions locked.
        let locked = model.project(row.hostId, row.projectId).map { $0.availability != .ready } ?? false
        let mode = locked
            ? InspectorMode.readOnlyNoDecide
            : InspectorMode.make(decide: model.capabilities[row.hostId]?.decide ?? false, kind: item.kind)
        let detail = model.detail(row)
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                header(row, item)
                if locked {
                    Banner(tone: .grey, text: "The project is not ready, so decisions are locked until it answers again")
                } else if let hint = mode.terminalHint {
                    Banner(tone: .grey, text: hint)
                }
                if item.needsTerminal && mode == .interactive {
                    Banner(tone: .grey, text: "This item can only be answered in a terminal")
                }
                kindBody(row, item, detail, mode)
                if showOutcome {
                    OutcomeView(
                        outcome: outcome, hostName: hostName(row.hostId),
                        onRefresh: { model.refresh(hostId: row.hostId, projectIds: [row.projectId]) },
                        onRetry: { id in retry(id) },
                        onTryAgain: { if let lastVerb { run(lastVerb, row) } })
                }
            }
            .padding(Tokens.space[3])
        }
        .sheet(isPresented: $confirmDeny) {
            ConfirmSheet(
                title: "Deny this command?", detail: detail?.operatorBody?.command ?? item.summary,
                noteLimit: ProtocolLimits.reasonCap, destructiveLabel: "Deny",
                onConfirm: { note in run(.opDeny(note: note), row) })
        }
        .sheet(item: Binding(get: { linkToConfirm.map(LinkItem.init) }, set: { linkToConfirm = $0?.text })) { link in
            DesignLinkConfirm(link: link.text)
        }
    }

    private struct LinkItem: Identifiable {
        let text: UntrustedText
        var id: String { text.raw }
    }

    private func header(_ row: InboxRowID, _ item: StoredWaiting) -> some View {
        let project = model.project(row.hostId, row.projectId)
        let name = TextBudget.displayPrefix(project?.displayName ?? row.projectId, maxBytes: 64).text
        return VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[0] + 2) {
                Image(systemName: item.kind.glyph).foregroundStyle(Tokens.color(.accent))
                Text(verbatim: RowText.kindName(item.kind) + " · " + name)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                ForEach(
                    RowPills.make(
                        needsTerminal: item.needsTerminal, forwarded: model.forwardShown(row),
                        stale: project?.staleSince != nil), id: \.word
                ) { pill in
                    StatePill(word: pill.word, symbol: pillSymbol(pill), tone: pillTone(pill))
                }
            }
            Text(verbatim: RowText.inspectorTitle(kind: item.kind, key: item.key, summary: item.summary.displayPlain))
                .font(.system(size: 20, weight: .semibold))
            MetaList(rows: meta(row, item))
        }
    }

    private func meta(_ row: InboxRowID, _ item: StoredWaiting) -> [MetaRow] {
        func plain(_ s: String) -> UntrustedText { UntrustedText(raw: s, verified: true, truncated: false) }
        var rows = [MetaRow(label: "Host", value: plain(hostName(row.hostId)))]
        let age = AgeText.short(since: item.since, now: Date())
        rows.append(MetaRow(label: "Proposed", value: plain(age.isEmpty ? item.since : age + " ago")))
        rows.append(MetaRow(label: "Request", value: plain(item.requestId)))
        return rows
    }

    private func pillSymbol(_ pill: Pill) -> String {
        switch pill {
        case .forwarded: return "arrow.right.circle.fill"
        case .terminalOnly: return "terminal.fill"
        case .stale: return "clock.fill"
        }
    }

    private func pillTone(_ pill: Pill) -> Tone {
        switch pill {
        case .forwarded: return .blue
        case .terminalOnly: return .grey
        case .stale: return .yellow
        }
    }

    private func hostName(_ hostId: UUID) -> String {
        model.hosts.first { $0.id == hostId }?.displayName ?? "the host"
    }

    // MARK: Bodies

    @ViewBuilder
    private func kindBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        switch item.kind {
        case .op: opBody(row, item, detail, mode)
        case .prompt: promptBody(row, item, detail, mode)
        case .design: designBody(row, item, detail, mode)
        case .message: messageBody(row, item, detail, mode)
        case .integration: integrationBody(row, item)
        case .other: summaryOnly(item)
        }
    }

    private func integrationBody(_ row: InboxRowID, _ item: StoredWaiting) -> some View {
        IntegrationDetailsView(rows: IntegrationDetailRows.make(item, hostName: hostName(row.hostId), now: Date()))
    }

    private func summaryOnly(_ item: StoredWaiting) -> some View {
        UntrustedTextBlock(text: item.summary, label: "From the host · untrusted text")
    }

    private func notReceived(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
    }

    private func opBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        let command = detail?.operatorBody?.command
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if let command {
                UntrustedTextBlock(text: command, label: "Command · untrusted text, shown as written")
            } else {
                UntrustedTextBlock(text: item.summary, label: "Summary")
                Text("Full command not received yet").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            if mode == .interactive {
                let canApprove = (command?.verified ?? false) && isAvailable(.opApprove, row)
                Text(verbatim: "Deny is applied on the host now. Approve goes to the project manager, who confirms with you and runs it.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                HStack {
                    Spacer()
                    Button { confirmDeny = true } label: { Text("Deny…") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(sending || !isAvailable(.opDeny(note: nil), row))
                    Button { run(.opApprove, row) } label: { Text("Send approval to PM") }
                        .buttonStyle(.capstanPrimary)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(sending || !canApprove)
                }
                if !canApprove, command == nil {
                    notReceived("Approve is available once the full command has arrived")
                }
            }
        }
    }

    private func promptBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        let prompt = detail?.promptBody
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            UntrustedTextBlock(text: prompt?.text ?? item.summary, label: "Prompt · untrusted text")
            if let prompt {
                ForEach(OptionRows.make(prompt.options, needsTerminal: item.needsTerminal)) { option in
                    optionRow(option, prompt: prompt, row: row, mode: mode)
                }
                if mode == .interactive {
                    HStack {
                        Spacer()
                        Button { run(.promptEsc, row) } label: { Text("Escape") }
                            .buttonStyle(.capstanQuiet)
                            .disabled(sending || !isAvailable(.promptEsc, row))
                    }
                }
            } else {
                notReceived("Options not received yet")
            }
        }
    }

    private func optionRow(_ option: OptionRow, prompt: PromptBody, row: InboxRowID, mode: InspectorMode) -> some View {
        let source = prompt.options.first { $0.number == option.number }
        let enabled = mode == .interactive && option.state == .selectable && !sending
            && isAvailable(.promptOption(Int(option.number)), row, option: source)
        return Button {
            run(.promptOption(Int(option.number)), row)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline) {
                    Text(verbatim: String(option.number)).font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: TextBudget.displayPrefix(option.text.displayPlain, maxBytes: ProtocolLimits.optionTextCap).text)
                        .font(.system(size: 13))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if let note = option.note {
                    Text(verbatim: note).font(.system(size: 12)).foregroundStyle(Tokens.color(.yellow))
                }
            }
            .padding(Tokens.space[1])
            .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func designBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        let design = detail?.designBody
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            UntrustedTextBlock(text: item.summary, label: "Screens · untrusted text")
            if let design {
                UntrustedTextBlock(text: design.link, label: "Link · untrusted, not opened until you choose")
                HStack {
                    Button { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(design.link.raw, forType: .string) } label: { Text("Copy link") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(!design.link.verified)
                    Button { linkToConfirm = design.link } label: { Text("Open in browser…") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(!LinkCheck.isOpenable(design.link.raw) || !design.link.verified)
                }
                if mode == .interactive {
                    DesignDecision(
                        link: design.link, enabled: !sending, approveAvailable: isAvailable(.designApprove(link: "x", askedUser: "Approved"), row),
                        changesAvailable: isAvailable(.designChanges(askedUser: "x", link: nil), row),
                        onApprove: { run(.designApprove(link: design.link.raw, askedUser: "Approved"), row) },
                        onChanges: { text in run(.designChanges(askedUser: text, link: nil), row) })
                }
            } else {
                notReceived("The design link has not been received yet")
            }
        }
    }

    private func messageBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        let message = detail?.messageBody
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            UntrustedTextBlock(text: message?.body ?? item.summary, label: "Message · untrusted text")
            if message == nil {
                notReceived("Full message not received yet")
            }
            if mode == .interactive, message != nil {
                ReplyComposer(
                    title: "Your reply", sendLabel: "Send reply", enabled: !sending && isAvailable(.messageReply("x"), row),
                    onSend: { text in run(.messageReply(text), row) })
            }
        }
    }

    // MARK: Deciding

    private func isAvailable(_ verb: DecideVerb, _ row: InboxRowID, option: PromptOption? = nil) -> Bool {
        model.availability(verb, row: row, option: option) == .available
    }

    private func run(_ verb: DecideVerb, _ row: InboxRowID) {
        guard !sending else { return }
        lastVerb = verb
        sending = true
        outcome = nil
        showOutcome = true
        Task {
            let result = await model.decide(verb, on: row)
            if self.row == row {
                outcome = result
                sending = false
            }
        }
    }

    private func retry(_ pendingId: UUID) {
        guard !sending, let row else { return }
        sending = true
        outcome = nil
        Task {
            let result = await model.retry(pendingId)
            if self.row == row {
                outcome = result
                sending = false
            }
        }
    }
}

/// A text box with a byte counter; the send button is on only while the text fits the cap.
struct ReplyComposer: View {
    let title: String
    let sendLabel: String
    let enabled: Bool
    let onSend: (String) -> Void

    @State private var text = ""
    private let budget = TextBudget(limit: ProtocolLimits.reasonCap)

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Tokens.color(.label2))
            TextEditor(text: $text)
                .font(.system(size: 13))
                .frame(minHeight: 72, maxHeight: 140)
                .scrollContentBackground(.hidden)
                .padding(Tokens.space[0])
                .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
            HStack {
                Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                Spacer()
                Button {
                    if let clean = NoteText.normalized(text) { onSend(clean) }
                } label: {
                    Text(verbatim: sendLabel)
                }
                .buttonStyle(.capstanPrimary)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(!enabled || state != .ready)
            }
        }
    }
}

/// Approve, or ask for changes with a text.
private struct DesignDecision: View {
    let link: UntrustedText
    let enabled: Bool
    let approveAvailable: Bool
    let changesAvailable: Bool
    let onApprove: () -> Void
    let onChanges: (String) -> Void

    @State private var asking = false

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if asking {
                ReplyComposer(title: "What should change?", sendLabel: "Send changes", enabled: enabled && changesAvailable, onSend: onChanges)
            }
            HStack {
                Spacer()
                Button { asking.toggle() } label: { Text(verbatim: asking ? "Hide the box" : "Request changes") }
                    .buttonStyle(.capstanQuiet)
                    .disabled(!changesAvailable)
                Button(action: onApprove) { Text("Approve design") }
                    .buttonStyle(.capstanPrimary)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!enabled || !approveAvailable || !link.verified || !LinkCheck.isOpenable(link.raw))
            }
        }
    }
}

/// The outcome line the person selected under Recent outcomes: what happened, when the host says and when this Mac
/// received it. It is made of ids and states; the free text of the event is not shown.
struct OutcomeInspectorView: View {
    let outcome: OverviewOutcome
    let projectName: String
    let hostName: String

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                HStack(spacing: Tokens.space[0] + 2) {
                    Image(systemName: outcome.symbol).foregroundStyle(Tokens.color(.accent))
                    Text(verbatim: "Outcome · " + projectName)
                        .font(.system(size: 12))
                        .foregroundStyle(Tokens.color(.label2))
                }
                Text(verbatim: outcome.text)
                    .font(.system(size: 20, weight: .semibold))
                    .fixedSize(horizontal: false, vertical: true)
                Grid(alignment: .topLeading, horizontalSpacing: Tokens.space[2], verticalSpacing: Tokens.space[1]) {
                    row("Host", hostName)
                    row("Type", outcome.type)
                    row("Happened (host)", outcome.hostTime)
                    row("Received (this Mac)", outcome.receivedTime)
                    row("Source", outcome.isLive ? "Live stream" : "History read")
                }
                if outcome.isSkewed {
                    Banner(tone: .grey, text: "The host's clock is ahead of this Mac, so no age is shown")
                }
            }
            .padding(Tokens.space[3])
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(verbatim: label).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: value).font(.system(size: 13)).textSelection(.enabled)
        }
    }
}

/// The inspector of a selected project when no row is selected: its status and what the overview counts.
struct ProjectSummaryView: View {
    let content: OverviewModel
    let projectName: String
    let hostName: String

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                Text(verbatim: "Project · " + hostName)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                Text(verbatim: projectName).font(.system(size: 20, weight: .semibold))
                Text(verbatim: content.statusLine)
                    .font(.system(size: 13))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityValue(content.accessibilityValue)
                Grid(alignment: .topLeading, horizontalSpacing: Tokens.space[2], verticalSpacing: Tokens.space[1]) {
                    row("Agents", content.totalAgents == 0 ? content.agentsEmptyText : content.countsText)
                    row("Plans", content.plans.isEmpty ? content.plansEmptyText : "\(content.plans.count) in progress")
                    row("Outcomes", content.historyText ?? content.outcomesEmptyText ?? "None yet")
                }
                if let fact = content.quietFact {
                    Text(verbatim: fact).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                }
            }
            .padding(Tokens.space[3])
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(verbatim: label).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: value).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
        }
    }
}

#if DEBUG
private struct InspectorPreview: View {
    @State private var model: AppModel
    let key: String

    init(key: String, decide: Bool = true) {
        _model = State(initialValue: InboxPreviewFixtures.model(decide: decide))
        self.key = key
    }

    var body: some View {
        InspectorScreen(model: model, row: InboxPreviewFixtures.rowID(key))
            .frame(width: 360, height: 560)
    }
}

#Preview("op") { InspectorPreview(key: "op") }
#Preview("op, 4096 bytes") { InspectorPreview(key: "op-long") }
#Preview("op, failed integrity") { InspectorPreview(key: "op-bad") }
#Preview("op, no detail yet") { InspectorPreview(key: "op-nodetail") }
#Preview("op, needs terminal") { InspectorPreview(key: "op-terminal") }
#Preview("prompt") { InspectorPreview(key: "prompt") }
#Preview("prompt, needs terminal") { InspectorPreview(key: "prompt-terminal") }
#Preview("design") { InspectorPreview(key: "design") }
#Preview("message") { InspectorPreview(key: "message") }
#Preview("message, 4096 bytes") { InspectorPreview(key: "message-long") }
#Preview("message, failed integrity") { InspectorPreview(key: "message-bad") }
#Preview("integration, read only") { InspectorPreview(key: "merge") }
#Preview("host cannot decide") { InspectorPreview(key: "op", decide: false) }
#Preview("nothing selected") {
    InspectorScreen(model: InboxPreviewFixtures.model(), row: nil).frame(width: 360, height: 200)
}
#Preview("OutcomeInspector20") {
    if let outcome = OverviewSamples.outcome {
        OutcomeInspectorView(outcome: outcome, projectName: "capstan", hostName: "Studio").frame(width: 360, height: 360)
    }
}
#endif
