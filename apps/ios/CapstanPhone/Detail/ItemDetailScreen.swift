import CapstanAppCore
import CapstanKit
import SwiftUI
import UIKit

/// What a details screen knows about its item, read from the model once per render.
struct DetailContext {
    let row: InboxRowID
    let item: StoredWaiting
    let detail: ItemDetail?
    let project: ProjectModel?
    let hostName: String
    let projectName: String
    let mode: InspectorMode
    /// The project is ready (not silent, refused, gone…).
    let ready: Bool
    /// The host's stream is live.
    let live: Bool

    /// Decisions can be taken from here.
    var answerable: Bool { mode == .interactive && ready && live }

    /// The main host text of the item and its name; the snapshot summary until the full event has arrived.
    var mainText: (text: UntrustedText, name: String, received: Bool) {
        switch item.kind {
        case .op:
            if let command = detail?.operatorBody?.command { return (command, "Command", true) }
            return (item.summary, "Summary", false)
        case .prompt:
            if let prompt = detail?.promptBody { return (prompt.text, "Prompt", true) }
            return (item.summary, "Summary", false)
        case .message:
            if let message = detail?.messageBody { return (message.body, "Message", true) }
            // A question an agent sent the person (`cstan send --action @user`) comes with the `messages` section.
            if let asked = project?.messages?.values.first(where: { $0.requestId == item.requestId }) {
                return (asked.body, "Message", true)
            }
            return (item.summary, "Summary", false)
        case .design:
            return (item.summary, "Screens", true)
        case .integration, .other:
            return (item.summary, "Summary", true)
        }
    }
}

/// Item details (boards 03 and 04): the facts of a proposal, prompt, design, message or branch ready to merge, its
/// host text as untrusted plain text, and the decisions in a bottom action bar. Approve and prompt answers go to the
/// PM after a confirmation; Deny confirms in a sheet with Cancel as the default. A text that is cut or failed its
/// check is never approved from here: the bar offers only "Copy what is shown".
struct ItemDetailScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dynamicTypeSize) private var typeSize
    let row: InboxRowID

    @State private var outcome: DecideOutcome?
    @State private var sending = false
    @State private var lastVerb: DecideVerb?
    @State private var confirmDeny = false
    @State private var forward: Forward?
    @State private var selected: Int64?
    @State private var link: LinkItem?
    @State private var askingChanges = false
    @State private var reply = ""
    @State private var receivedAt = Date()
    @State private var toast = false
    @State private var merging = false

    var body: some View {
        if let item = model.item(row) {
            content(context(item))
        } else {
            gone
        }
    }

    private func context(_ item: StoredWaiting) -> DetailContext {
        let project = model.project(row.hostId, row.projectId)
        let ready = project.map { $0.availability == .ready } ?? false
        return DetailContext(
            row: row, item: item, detail: model.detail(row), project: project,
            hostName: model.hosts.first { $0.id == row.hostId }?.displayName ?? "the host",
            projectName: TextBudget.displayPrefix(project?.displayName ?? row.projectId, maxBytes: 64).text,
            mode: InspectorMode.make(decide: model.capabilities[row.hostId]?.decide ?? false, kind: item.kind),
            ready: ready, live: model.connectionIsLive(row.hostId))
    }

    // MARK: Screen

    private func content(_ ctx: DetailContext) -> some View {
        let main = ctx.mainText
        let shown = ShownText(main.text)
        let bar = DetailBar.make(
            ctx, shown: shown, outcome: sending ? nil : outcome,
            mergeable: ctx.item.kind == .integration && isAvailable(.integrationMerge))
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                DetailHeader(
                    kind: ctx.item.kind, projectName: ctx.projectName,
                    title: title(ctx),
                    pills: RowPills.make(
                        needsTerminal: ctx.item.needsTerminal, forwarded: model.forwardShown(row),
                        stale: ctx.project?.staleSince != nil))
                if let outcome, !sending {
                    OutcomeBanner(
                        outcome: outcome, hostName: ctx.hostName, receivedAt: receivedAt,
                        onRetry: { id in retry(id) },
                        onTryAgain: tryAgain)
                }
                if ctx.item.kind != .integration {
                    DetailFacts(facts: facts(ctx))
                }
                notices(ctx, shown: shown)
                kindBody(ctx, main: main, shown: shown)
                if ctx.item.kind == .integration, bar != .merge, let reason = mergeBlocked(ctx) {
                    footnote("Merge is not offered: " + reason)
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .outcomeToast($toast)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .primaryAction) { moreMenu(ctx, shown: shown) } }
        .toolbar(bar == .empty ? .automatic : .hidden, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            if bar != .empty {
                actionBar(bar, ctx, shown: shown)
            }
        }
        .sheet(isPresented: $confirmDeny) {
            ConfirmSheet(
                title: "Deny this command?", detail: main.text, noteLimit: ProtocolLimits.reasonCap,
                destructiveLabel: "Deny", detailLabel: "Command · " + ctx.projectName,
                noteLabel: "Note to the operator (optional)",
                footnote: "Deny is applied on the host at once and cannot be taken back from here.",
                onConfirm: { note in run(.opDeny(note: note)) })
        }
        .sheet(item: $forward) { item in
            ForwardConfirmSheet(title: item.title, text: item.text, confirmLabel: "Send to PM") { run(item.verb) }
        }
        .sheet(item: $link) { item in
            DesignLinkConfirm(link: item.text)
        }
        .sheet(isPresented: $merging) {
            MergeSheet(row: row, item: ctx.item)
        }
        .sheet(isPresented: $askingChanges) {
            ChangesSheet { text in run(.designChanges(askedUser: text, link: nil)) }
        }
    }

    /// A branch ready to merge says it is built and not in main yet (never a bare "merged"); other kinds use the
    /// inspector title shared with the Mac.
    private func title(_ ctx: DetailContext) -> String {
        if ctx.item.kind == .integration {
            return IntegrationDetailRows.make(ctx.item, hostName: ctx.hostName, now: Date()).title
        }
        return RowText.inspectorTitle(kind: ctx.item.kind, key: ctx.item.key, summary: ctx.item.summary.displayPlain)
    }

    /// The item was answered or withdrawn: it left the snapshot. A decision taken here shows its result.
    @ViewBuilder
    private var gone: some View {
        if let outcome {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[3]) {
                    Text("No longer waiting").font(TypeStyle.title2)
                    OutcomeBanner(outcome: outcome, hostName: hostName, receivedAt: receivedAt)
                }
                .padding(Tokens.margin)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Tokens.color(.ground))
            .outcomeToast($toast)
        } else {
            ContentUnavailableView(
                "No longer waiting", systemImage: "checkmark.circle",
                description: Text("It was answered or withdrawn on the host."))
            .background(Tokens.color(.ground))
        }
    }

    private var hostName: String {
        model.hosts.first { $0.id == row.hostId }?.displayName ?? "the host"
    }

    // MARK: Facts and notices

    private func facts(_ ctx: DetailContext) -> [DetailFact] {
        let asked = DetailTime.asked(ctx.item.since, now: Date())
        let request = DetailFact(
            label: "Request", value: ctx.item.key + " · " + String(ctx.item.contentHash.prefix(12)), mono: true)
        switch ctx.item.kind {
        case .op:
            return [DetailFact(label: "Host", value: ctx.hostName), DetailFact(label: "Proposed", value: asked), request]
        case .prompt:
            var rows: [DetailFact] = []
            if let agentId = ctx.detail?.promptBody?.agentId {
                let role = ctx.project?.agents.values.first { $0.agentId == agentId }?.role
                rows.append(DetailFact(label: "Agent", value: role.map { agentId + " · " + $0 } ?? agentId, mono: true))
            }
            return rows + [DetailFact(label: "Asked", value: asked), request]
        case .design:
            var rows: [DetailFact] = []
            if let design = ctx.detail?.designBody {
                rows.append(DetailFact(label: "Package", value: design.planId + " / " + design.packageId, mono: true))
                rows.append(DetailFact(label: "Report", value: design.reportId, mono: true))
            }
            return rows + [DetailFact(label: "Asked", value: asked)]
        case .message:
            return [DetailFact(label: "Host", value: ctx.hostName), DetailFact(label: "Sent", value: asked), request]
        case .integration, .other:
            return [DetailFact(label: "Host", value: ctx.hostName), DetailFact(label: "Since", value: asked), request]
        }
    }

    @ViewBuilder
    private func notices(_ ctx: DetailContext, shown: ShownText) -> some View {
        if !ctx.ready {
            DetailNotice(
                tone: .grey, symbol: "lock", title: "Decisions are locked",
                text: "The project is not ready, so nothing can be answered until it answers again.")
        } else if !ctx.live {
            DetailNotice(
                tone: .yellow, symbol: "wifi.exclamationmark", title: "Not connected",
                text: "This is the item as of the last snapshot. It can be answered once the host is live again.")
        } else if ctx.mode == .readOnlyNoDecide {
            DetailNotice(
                tone: .grey, symbol: "apple.terminal", title: "Answer in the terminal",
                text: "This host's Capstan cannot take decisions from the app.")
        } else if ctx.item.needsTerminal, ctx.item.kind != .prompt, ctx.mode == .interactive {
            DetailNotice(
                tone: .grey, symbol: "apple.terminal", title: "Answer in the terminal",
                text: "This item can only be answered in a terminal.")
        }
        if !shown.verified {
            DetailNotice(
                tone: .yellow, symbol: "exclamationmark.triangle.fill", title: "Text failed its integrity check",
                text: Self.failedText(ctx.item.kind))
        }
    }

    static func failedText(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "The command does not match its hash. Answer it in the terminal."
        case .prompt: return "The prompt does not match its hash. Answer it in the terminal."
        case .message: return "The message does not match its hash. Read it in the terminal."
        default: return "The text does not match its hash. Read it on the host."
        }
    }

    // MARK: Bodies

    @ViewBuilder
    private func kindBody(_ ctx: DetailContext, main: (text: UntrustedText, name: String, received: Bool), shown: ShownText) -> some View {
        switch ctx.item.kind {
        case .integration:
            IntegrationDetail(rows: IntegrationDetailRows.make(ctx.item, hostName: ctx.hostName, now: Date()))
        case .prompt:
            promptBody(ctx, main: main, shown: shown)
        case .design:
            designBody(ctx, main: main)
        default:
            DetailTextBlock(text: main.text, name: main.name)
            if shown.cut { CutTextNote() }
            if !main.received {
                waitingFor(ctx.item.kind == .op ? "The full command has not arrived yet." : "The full message has not arrived yet.")
            }
            if ctx.item.kind == .op, ctx.answerable, shown.whole, main.received {
                footnote("Deny is applied on the host at once. Approve goes to the PM, who confirms with you and runs it.")
            }
            if ctx.item.kind == .message, ctx.answerable, main.received {
                footnote("Your reply goes to the host as plain text, up to " + ProtocolLimits.reasonCap.formatted() + " bytes.")
            }
        }
    }

    @ViewBuilder
    private func promptBody(_ ctx: DetailContext, main: (text: UntrustedText, name: String, received: Bool), shown: ShownText) -> some View {
        if let prompt = ctx.detail?.promptBody {
            let rows = OptionRows.make(prompt.options, needsTerminal: ctx.item.needsTerminal)
            if rows.allTerminal {
                DetailNotice(
                    tone: .grey, symbol: "apple.terminal", title: "Answer in the terminal",
                    text: "Every option widens permissions or takes text, so none can be sent from here.")
            }
            DetailTextBlock(text: main.text, name: main.name)
            if shown.cut { CutTextNote() }
            if shown.verified {
                PromptOptionList(rows: rows, selected: $selected, enabled: ctx.answerable && shown.whole && !sending)
                    .onAppear { if selected == nil { selected = rows.firstSelectable } }
            }
        } else {
            DetailTextBlock(text: main.text, name: main.name)
            waitingFor("The options have not arrived yet.")
        }
    }

    @ViewBuilder
    private func designBody(_ ctx: DetailContext, main: (text: UntrustedText, name: String, received: Bool)) -> some View {
        DetailTextBlock(text: main.text, name: main.name, shownAs: "shown as written")
        if let design = ctx.detail?.designBody {
            DetailTextBlock(text: design.link, name: "Link", shownAs: "shown as text", symbol: "link")
            Button {
                link = LinkItem(text: design.link)
            } label: {
                Text("Open in Safari…").padding(.horizontal, Tokens.space[1])
            }
            .buttonStyle(.capstanQuiet)
            .fixedSize()
            .disabled(!design.link.verified || !LinkCheck.isOpenable(design.link.raw))
        } else {
            waitingFor("The design link has not arrived yet.")
        }
    }

    private func waitingFor(_ text: String) -> some View {
        Text(verbatim: text).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
    }

    private func footnote(_ text: String) -> some View {
        Text(verbatim: text)
            .font(TypeStyle.footnote)
            .foregroundStyle(Tokens.color(.label2))
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: Toolbar and action bar

    private func moreMenu(_ ctx: DetailContext, shown: ShownText) -> some View {
        Menu {
            if shown.any {
                Button { copy(shown.text) } label: {
                    if ctx.item.kind == .op {
                        Label("Copy command", systemImage: "doc.on.doc")
                    } else {
                        Label("Copy text", systemImage: "doc.on.doc")
                    }
                }
            }
            Button { openProject() } label: { Label("Open project", systemImage: "folder") }
            if ctx.item.kind == .op, ctx.answerable {
                Divider()
                Button(role: .destructive) { confirmDeny = true } label: { Label("Deny…", systemImage: "xmark.octagon") }
                    .disabled(sending || !isAvailable(.opDeny(note: nil)))
            }
        } label: {
            Label("More actions", systemImage: "ellipsis")
        }
    }

    @ViewBuilder
    private func actionBar(_ bar: DetailBar, _ ctx: DetailContext, shown: ShownText) -> some View {
        let stacked = typeSize.isAccessibilitySize
        Group {
            switch bar {
            case .empty:
                EmptyView()
            case .copy:
                Button { copy(shown.text) } label: { Text("Copy what is shown") }
                    .buttonStyle(.capstanQuiet)
            case .showNew:
                Button { showNewVersion() } label: { Text("Show the new version") }
                    .buttonStyle(.capstanPrimary)
            case .merge:
                Button { merging = true } label: { Text("Merge…") }
                    .buttonStyle(.capstanPrimary)
                    .keyboardShortcut(.return, modifiers: .command)
            case .op:
                BarLayout(stacked: stacked) {
                    Button { confirmDeny = true } label: { Text("Deny…") }
                        .buttonStyle(.capstanDestructive)
                        .disabled(sending || !isAvailable(.opDeny(note: nil)))
                } primary: {
                    Button { forwardApproval() } label: {
                        if sending { SendingLabel() } else { Text("Send approval to PM") }
                    }
                        .buttonStyle(.capstanPrimary)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(sending || !isAvailable(.opApprove))
                }
            case .prompt(let allTerminal):
                promptBar(ctx, allTerminal: allTerminal, stacked: stacked)
            case .design:
                designBar(ctx, stacked: stacked)
            case .reply:
                ReplyBar(
                    text: $reply,
                    options: ctx.project?.messages?.values.first { $0.requestId == ctx.item.requestId }?.options ?? [],
                    enabled: !sending && isAvailable(.messageReply("x"))
                ) { text in
                    run(.messageReply(text))
                }
            }
        }
        .padding(.horizontal, Tokens.margin)
        .padding(.top, Tokens.space[2])
        .padding(.bottom, Tokens.space[1])
        .background(Tokens.color(.ground))
    }

    @ViewBuilder
    private func promptBar(_ ctx: DetailContext, allTerminal: Bool, stacked: Bool) -> some View {
        let esc = Button { forwardEsc(ctx) } label: { Text("Send Esc") }
            .buttonStyle(.capstanQuiet)
            .disabled(sending || !isAvailable(.promptEsc))
        if allTerminal {
            esc
        } else {
            let option = selected.flatMap { number in ctx.detail?.promptBody?.options.first { $0.number == number } }
            BarLayout(stacked: stacked) {
                esc
            } primary: {
                Button { forwardOption(ctx) } label: {
                    if sending {
                        SendingLabel()
                    } else if let number = selected {
                        Text(verbatim: "Send option " + String(number) + " to PM")
                    } else {
                        Text("Choose an option")
                    }
                }
                .buttonStyle(.capstanPrimary)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(sending || option == nil || !isAvailable(.promptOption(Int(selected ?? 0)), option: option))
            }
        }
    }

    private func designBar(_ ctx: DetailContext, stacked: Bool) -> some View {
        let design = ctx.detail?.designBody
        let approvable = design.map { $0.link.verified && LinkCheck.isOpenable($0.link.raw) } ?? false
        return BarLayout(stacked: stacked) {
            Button { askingChanges = true } label: { Text("Request changes…") }
                .buttonStyle(.capstanQuiet)
                .disabled(sending || !isAvailable(.designChanges(askedUser: "x", link: nil)))
        } primary: {
            Button {
                if let design { run(.designApprove(link: design.link.raw, askedUser: "Approved")) }
            } label: {
                if sending { SendingLabel() } else { Text("Approve") }
            }
            .buttonStyle(.capstanPrimary)
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(sending || !approvable || !isAvailable(.designApprove(link: "x", askedUser: "Approved")))
        }
    }

    // MARK: Actions

    private func forwardApproval() {
        forward = Forward(
            verb: .opApprove, title: "Send the approval to the PM?",
            text: "The PM asks you to confirm, then runs the command. Nothing runs until then.")
    }

    private func forwardOption(_ ctx: DetailContext) {
        guard let number = selected else { return }
        forward = Forward(
            verb: .promptOption(Int(number)), title: "Send option " + String(number) + " to the PM?",
            text: "The PM asks you to confirm, then answers the prompt in " + agentTerminal(ctx)
                + ". Nothing is answered until then.")
    }

    private func forwardEsc(_ ctx: DetailContext) {
        forward = Forward(
            verb: .promptEsc, title: "Send Esc to the PM?",
            text: "The PM asks you to confirm, then sends Esc to " + agentTerminal(ctx)
                + ", which cancels the prompt. Nothing is sent until then.")
    }

    private func agentTerminal(_ ctx: DetailContext) -> String {
        ctx.detail?.promptBody.map { $0.agentId + "'s terminal" } ?? "the agent's terminal"
    }

    /// Why an integration shows no Merge button, in the order the checks run; nil when nothing blocks it.
    private func mergeBlocked(_ ctx: DetailContext) -> String? {
        if model.capabilities[row.hostId] == nil { return "the host has not been checked yet." }
        if case .hidden(let why) = model.availability(.integrationMerge, row: row, option: nil) {
            return why.hasSuffix(".") ? why : why + "."
        }
        if case .terminalOnly(let why) = model.availability(.integrationMerge, row: row, option: nil) {
            return why.hasSuffix(".") ? why : why + "."
        }
        if !ctx.ready { return "the project is not ready on the host." }
        if !ctx.live { return "the connection to the host is not live." }
        return nil
    }

    private func isAvailable(_ verb: DecideVerb, option: PromptOption? = nil) -> Bool {
        model.availability(verb, row: row, option: option) == .available
    }

    private func run(_ verb: DecideVerb) {
        guard !sending else { return }
        lastVerb = verb
        sending = true
        outcome = nil
        Task {
            let result = await model.decide(verb, on: row)
            received(result)
        }
    }

    private func retry(_ pendingId: UUID) {
        guard !sending else { return }
        sending = true
        outcome = nil
        Task {
            let result = await model.retry(pendingId)
            received(result)
        }
    }

    /// Sends the last decision again with a new request; nil when there is none.
    private var tryAgain: (() -> Void)? {
        guard let lastVerb else { return nil }
        return { run(lastVerb) }
    }

    /// Shows the result, fires its haptic, and announces a new forward with the toast.
    private func received(_ result: DecideOutcome) {
        receivedAt = Date()
        outcome = result
        sending = false
        if OutcomeLines.isNewForward(result) { toast = true }
        switch OutcomeLines.feel(result) {
        case .success?: Haptics.play(.success)
        case .warning?: Haptics.play(.warning)
        case .error?: Haptics.play(.error)
        case nil: break
        }
    }

    /// After `hash_mismatch`: read the project again and clear the result, so the new text is what is answered.
    private func showNewVersion() {
        model.refresh(hostId: row.hostId, projectIds: [row.projectId])
        outcome = nil
        lastVerb = nil
    }

    private func copy(_ text: String) {
        UIPasteboard.general.string = text
        Haptics.play(.success)
    }

    private func openProject() {
        let tab = model.navigation.tab
        model.navigation.show(tab, model.navigation.path(tab) + [.project(hostId: row.hostId, projectId: row.projectId)])
    }
}

/// The bottom bar a details screen shows.
enum DetailBar: Equatable {
    case empty
    /// The text is cut or failed its check: nothing is decided from here.
    case copy
    /// The item changed since it was opened (`hash_mismatch`).
    case showNew
    /// A built integration the host lets this phone merge (board 10).
    case merge
    case op
    case prompt(allTerminal: Bool)
    case design
    case reply

    static func make(_ ctx: DetailContext, shown: ShownText, outcome: DecideOutcome?, mergeable: Bool = false) -> DetailBar {
        if let outcome {
            if OutcomeLines.isDone(outcome) { return .empty }
            if case .refused(_, .refresh) = outcome { return .showNew }
        }
        let copyOrNone: DetailBar = shown.cut && shown.any ? .copy : .empty
        // An integration is read-only in the shared inspector (the Mac merges from the terminal); here the Merge button
        // decides, and `mergeable` already holds the host's decide capability and the built state.
        if case .integration = ctx.item.kind { return mergeable && ctx.ready && ctx.live ? .merge : copyOrNone }
        guard ctx.answerable else { return copyOrNone }
        switch ctx.item.kind {
        case .op:
            guard ctx.detail?.operatorBody != nil, shown.whole else { return copyOrNone }
            return .op
        case .prompt:
            guard let prompt = ctx.detail?.promptBody, shown.whole else { return copyOrNone }
            return .prompt(allTerminal: OptionRows.make(prompt.options, needsTerminal: ctx.item.needsTerminal).allTerminal)
        case .design:
            return shown.verified ? .design : .empty
        case .message:
            return ctx.mainText.received && shown.verified ? .reply : .empty
        case .integration:
            return mergeable ? .merge : .empty
        case .other:
            return .empty
        }
    }
}

/// A secondary and a primary button: side by side with the primary wider, stacked at accessibility sizes.
private struct BarLayout<Secondary: View, Primary: View>: View {
    let stacked: Bool
    @ViewBuilder let secondary: Secondary
    @ViewBuilder let primary: Primary

    var body: some View {
        if stacked {
            VStack(spacing: Tokens.space[2]) {
                primary
                secondary
            }
        } else {
            HStack(spacing: Tokens.space[2]) {
                secondary.fixedSize(horizontal: true, vertical: false)
                primary
            }
        }
    }
}

private struct Forward: Identifiable {
    let verb: DecideVerb
    let title: String
    let text: String
    var id: String { title }
}

private struct LinkItem: Identifiable {
    let text: UntrustedText
    var id: String { text.raw }
}

#if DEBUG
private struct DetailPreview: View {
    let key: String
    var hostId = PreviewData.alphaId

    var body: some View {
        NavigationStack {
            ItemDetailScreen(row: InboxRowID(hostId: hostId, projectId: "capstan", key: key))
        }
        .environment(AppModel(preview: PreviewData.standard))
    }
}

#Preview("Command proposal") { DetailPreview(key: "waiting-op") }
#Preview("Command, cut at 4096 bytes") { DetailPreview(key: "waiting-long") }
#Preview("Command, failed its check") { DetailPreview(key: "waiting-bad-op") }
#Preview("Prompt with options") { DetailPreview(key: "waiting-prompt") }
#Preview("Prompt, terminal only") { DetailPreview(key: "waiting-prompt-terminal") }
#Preview("Design approval") { DetailPreview(key: "waiting-design") }
#Preview("Message from the PM") { DetailPreview(key: "waiting-message") }
#Preview("Message, failed its check") { DetailPreview(key: "waiting-unverified") }
#Preview("Ready to merge") { DetailPreview(key: "waiting-merge") }
#Preview("No longer waiting") { DetailPreview(key: "gone") }
#Preview("Prompt, dark") { DetailPreview(key: "waiting-prompt").preferredColorScheme(.dark) }
#endif
