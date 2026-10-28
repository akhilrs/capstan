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
                Text("Select an item to see what it asks")
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onChange(of: row) { _, _ in
            outcome = nil
            sending = false
            showOutcome = false
            lastVerb = nil
        }
    }

    private func content(_ row: InboxRowID, _ item: StoredWaiting) -> some View {
        let mode = InspectorMode.make(decide: model.capabilities[row.hostId]?.decide ?? false, kind: item.kind)
        let detail = model.detail(row)
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                header(row, item)
                if let hint = mode.terminalHint {
                    Banner(tone: .yellow, text: hint)
                }
                if item.needsTerminal && mode == .interactive {
                    Banner(tone: .yellow, text: "This item can only be answered in a terminal")
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
        return VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: item.kind.groupTitle).font(.system(size: 15, weight: .semibold))
            HStack(spacing: Tokens.space[0]) {
                Text(verbatim: TextBudget.displayPrefix(project?.displayName ?? row.projectId, maxBytes: 64).text)
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
        }
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
        case .integration, .other: summaryOnly(item)
        }
    }

    private func summaryOnly(_ item: StoredWaiting) -> some View {
        UntrustedTextBlock(text: item.summary, label: "From the host")
    }

    private func notReceived(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
    }

    private func opBody(_ row: InboxRowID, _ item: StoredWaiting, _ detail: ItemDetail?, _ mode: InspectorMode) -> some View {
        let command = detail?.operatorBody?.command
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if let command {
                UntrustedTextBlock(text: command, label: "Command")
            } else {
                UntrustedTextBlock(text: item.summary, label: "Summary")
                Text("Full command not received yet").font(.system(size: 12)).foregroundStyle(Tokens.color(.yellow))
            }
            if mode == .interactive {
                let canApprove = (command?.verified ?? false) && isAvailable(.opApprove, row)
                HStack {
                    Button("Deny") { confirmDeny = true }
                        .disabled(sending || !isAvailable(.opDeny(note: nil), row))
                    Spacer()
                    Button("Approve") { run(.opApprove, row) }
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
            UntrustedTextBlock(text: prompt?.text ?? item.summary, label: "Prompt")
            if let prompt {
                ForEach(OptionRows.make(prompt.options, needsTerminal: item.needsTerminal)) { option in
                    optionRow(option, prompt: prompt, row: row, mode: mode)
                }
                if mode == .interactive {
                    HStack {
                        Spacer()
                        Button("Escape") { run(.promptEsc, row) }
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
            UntrustedTextBlock(text: item.summary, label: "Summary")
            if let design {
                UntrustedTextBlock(text: design.link, label: "Design link")
                Button("Show the full link\u{2026}") { linkToConfirm = design.link }
                    .disabled(!LinkCheck.isOpenable(design.link.raw) || !design.link.verified)
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
            UntrustedTextBlock(text: message?.body ?? item.summary, label: "Message")
            if message == nil {
                notReceived("Full message not received yet")
            }
            if mode == .interactive, message != nil {
                ReplyComposer(
                    title: "Reply", sendLabel: "Send reply", enabled: !sending && isAvailable(.messageReply("x"), row),
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
                Button { asking.toggle() } label: { Text(verbatim: asking ? "Hide the box" : "Request changes") }
                    .disabled(!changesAvailable)
                Spacer()
                Button("Approve", action: onApprove)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!enabled || !approveAvailable || !link.verified || !LinkCheck.isOpenable(link.raw))
            }
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
#endif
