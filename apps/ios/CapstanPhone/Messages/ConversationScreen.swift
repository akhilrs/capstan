import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// One project's conversation with its PM (board 11): what the agents sent the person (`cstan send @user`; host text,
/// untrusted, plain) and what this phone sent, each with its state: Sending, Sent (on the host), or Not sent with
/// Retry. A question that waits for an answer has Reply: the composer then answers that question (`message reply`)
/// instead of sending a new message. The composer sends plain text up to 500 bytes; it is never run as a command.
struct ConversationScreen: View {
    @Environment(AppModel.self) private var model
    let hostId: UUID
    let projectId: String

    @State private var text = ""
    @State private var sending = false
    /// The question the composer answers; nil when it sends a new message.
    @State private var answering: StoredMessage?

    private enum Entry: Identifiable {
        case received(StoredMessage)
        /// A message item from a host that sends no `messages` section.
        case incoming(StoredWaiting)
        case outgoing(SentMessage)

        var id: String {
            switch self {
            case .received(let message): return "msg/" + message.key
            case .incoming(let item): return "in/" + item.key
            case .outgoing(let message): return "out/" + message.id.uuidString
            }
        }

        var at: Date {
            switch self {
            case .received(let message): return DetailTime.parse(message.createdAt) ?? .distantPast
            case .incoming(let item): return DetailTime.parse(item.since) ?? .distantPast
            case .outgoing(let message): return message.sentAt
            }
        }
    }

    var body: some View {
        let project = model.project(hostId, projectId)
        let pm = project.flatMap(PmBinding.activePm)
        let entries = self.entries(project)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Tokens.space[3]) {
                    if entries.isEmpty {
                        Text(verbatim: pm.map { "Nothing said yet. Your message goes to " + $0 + " as plain text." }
                            ?? "This project has no active PM, so nothing can be sent.")
                            .font(TypeStyle.subheadline)
                            .foregroundStyle(Tokens.color(.label2))
                    }
                    ForEach(entries) { entry in
                        switch entry {
                        case .received(let message): received(message)
                        case .incoming(let item): incoming(item, pm: pm)
                        case .outgoing(let message): outgoing(message)
                        }
                    }
                }
                .padding(Tokens.margin)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onAppear { if let last = entries.last { proxy.scrollTo(last.id, anchor: .bottom) } }
            .onChange(of: entries.count) { _, _ in
                if let last = entries.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
        .background(Tokens.color(.ground))
        .navigationTitle(
            Text(verbatim: (project.map { TextBudget.displayPrefix($0.displayName, maxBytes: 64).text } ?? projectId)
                + (pm.map { " · " + $0 } ?? "")))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                if let answering {
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: "arrowshape.turn.up.left")
                            .foregroundStyle(Tokens.color(.accent))
                            .accessibilityHidden(true)
                        Text(verbatim: "Answering question " + String(answering.sequence) + " from " + answering.senderAgentId)
                            .font(TypeStyle.footnote)
                            .foregroundStyle(Tokens.color(.label2))
                        Spacer(minLength: Tokens.space[1])
                        Button("Cancel") { self.answering = nil }
                            .font(TypeStyle.footnote.weight(.semibold))
                            .frame(minHeight: 44)
                    }
                }
                MessageComposer(
                    text: $text, enabled: (pm != nil || answering != nil) && !sending,
                    placeholder: answering.map { "Answer " + $0.senderAgentId } ?? pm.map { "Message " + $0 }
                ) { body in
                    if let question = answering { answer(body, question) } else { send(body) }
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .background(Tokens.color(.ground))
        }
        .onChange(of: project?.messages?[answering?.key ?? ""]?.state) { _, state in
            // The question was answered elsewhere (the terminal, another phone): the composer sends messages again.
            if answering != nil, state != "open" { answering = nil }
        }
    }

    private func entries(_ project: ProjectModel?) -> [Entry] {
        let received = Array((project?.messages ?? [:]).values)
        let known = Set(received.map(\.requestId))
        let older = (project?.waiting.values.filter { $0.kind == .message && !known.contains($0.requestId) } ?? [])
        let outgoing = model.messages.thread(hostId: hostId, projectId: projectId).map(Entry.outgoing)
        return (received.map(Entry.received) + older.map(Entry.incoming) + outgoing).sorted { $0.at < $1.at }
    }

    /// A message from an agent: its text as written, who sent it and when, and Reply while it is an open question.
    private func received(_ message: StoredMessage) -> some View {
        let time = DetailTime.parse(message.createdAt)?.formatted(date: .omitted, time: .shortened) ?? ""
        let state: String
        switch (message.messageClass, message.state) {
        case ("action", "open"): state = "Asks you"
        case ("action", _): state = "Answered"
        default: state = "For your information"
        }
        return VStack(alignment: .leading, spacing: Tokens.space[1]) {
            DetailTextBlock(text: message.body, name: message.senderAgentId + " · " + state, shownAs: "shown as written")
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: time)
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                if message.isOpenQuestion {
                    Button {
                        answering = message
                    } label: {
                        Label("Reply", systemImage: "arrowshape.turn.up.left")
                    }
                    .font(TypeStyle.footnote.weight(.semibold))
                    .frame(minHeight: 44)
                    .disabled(sending)
                }
            }
        }
        .frame(maxWidth: 340, alignment: .leading)
        .id("msg/" + message.key)
    }

    private func incoming(_ item: StoredWaiting, pm: String?) -> some View {
        let detail = model.detail(InboxRowID(hostId: hostId, projectId: projectId, key: item.key))
        let body = detail?.messageBody?.body ?? item.summary
        return NavigationLink(value: PhoneRoute.item(InboxRowID(hostId: hostId, projectId: projectId, key: item.key))) {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                DetailTextBlock(text: body, name: (pm ?? "PM") + " · needs an answer", shownAs: "shown as written")
                Text(verbatim: DetailTime.asked(item.since, now: Date()) + " · open it to reply")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
            }
            .frame(maxWidth: 340, alignment: .leading)
        }
        .buttonStyle(.plain)
        .id("in/" + item.key)
    }

    private func outgoing(_ message: SentMessage) -> some View {
        VStack(alignment: .trailing, spacing: Tokens.space[1]) {
            Text(verbatim: message.text)
                .font(TypeStyle.body)
                .foregroundStyle(Tokens.color(.onAccent))
                .padding(.horizontal, Tokens.space[3])
                .padding(.vertical, Tokens.space[2])
                .background(
                    message.state == .sending ? Tokens.color(.accent).opacity(0.7) : Tokens.color(.accent),
                    in: RoundedRectangle(cornerRadius: 18))
                .textSelection(.enabled)
            stateLine(message)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, Tokens.space[6])
        .id("out/" + message.id.uuidString)
    }

    @ViewBuilder
    private func stateLine(_ message: SentMessage) -> some View {
        let time = message.sentAt.formatted(date: .omitted, time: .shortened)
        switch message.state {
        case .sending:
            Label { Text(verbatim: "Sending") } icon: { ProgressView().controlSize(.mini) }
                .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
        case .sent:
            Label { Text(verbatim: "Sent · on the host " + time) } icon: { Image(systemName: "checkmark") }
                .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
        case .notSent(let reason, _):
            HStack(spacing: Tokens.space[1]) {
                Label { Text(verbatim: "Not sent · " + reason) } icon: { Image(systemName: "exclamationmark.triangle") }
                    .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.red))
                Button("Retry") { retry(message) }
                    .font(TypeStyle.footnote.weight(.semibold))
                    .frame(minHeight: 44)
                    .disabled(sending)
            }
        }
    }

    private func send(_ body: String) {
        guard !sending else { return }
        sending = true
        text = ""
        Task {
            let outcome = await model.messagePM(body, hostId: hostId, projectId: projectId)
            sending = false
            feel(outcome)
        }
    }

    private func answer(_ body: String, _ question: StoredMessage) {
        guard !sending else { return }
        sending = true
        text = ""
        answering = nil
        Task {
            let outcome = await model.answerQuestion(body, question: question, hostId: hostId, projectId: projectId)
            sending = false
            feel(outcome)
        }
    }

    private func retry(_ message: SentMessage) {
        guard !sending else { return }
        sending = true
        Task {
            let outcome = await model.retryMessage(message)
            sending = false
            feel(outcome)
        }
    }

    private func feel(_ outcome: DecideOutcome) {
        switch outcome {
        case .applied, .forwarded: Haptics.play(.success)
        case .refused: Haptics.play(.warning)
        case .notSent, .malformed: Haptics.play(.error)
        }
    }
}

/// The message field and the send button: plain text up to 500 bytes; send is off while empty or over.
struct MessageComposer: View {
    @Binding var text: String
    let enabled: Bool
    /// The placeholder ("Message pm-1", "Answer pm-1"); nil when nothing can be sent.
    let placeholder: String?
    let onSend: (String) -> Void

    private let budget = TextBudget(limit: SentMessageLog.textCap)

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        VStack(alignment: .trailing, spacing: Tokens.space[1]) {
            if budget.used(text) * 5 >= budget.limit * 4 {
                Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                    .font(TypeStyle.footnote.monospacedDigit())
                    .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
            }
            HStack(alignment: .bottom, spacing: Tokens.space[2]) {
                TextField(text: $text, axis: .vertical) {
                    Text(verbatim: placeholder ?? "No active PM")
                }
                .lineLimit(1...5)
                .font(TypeStyle.body)
                .padding(.horizontal, Tokens.space[3])
                .padding(.vertical, Tokens.space[2])
                .frame(minHeight: 44)
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: 22))
                .disabled(!enabled)
                Button {
                    if let clean = NoteText.normalized(text) { onSend(clean) }
                } label: {
                    Image(systemName: "arrow.up")
                        .font(TypeStyle.headline)
                        .foregroundStyle(state == .ready && enabled ? Tokens.color(.onAccent) : Tokens.color(.label2))
                        .frame(width: 44, height: 44)
                        .background(state == .ready && enabled ? Tokens.color(.accent) : Tokens.color(.fill2), in: Circle())
                }
                .accessibilityLabel("Send message")
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(!enabled || state != .ready)
            }
        }
    }
}

#Preview("Conversation") {
    NavigationStack { ConversationScreen(hostId: PreviewData.alphaId, projectId: "capstan").routes() }
        .environment(AppModel(preview: PreviewData.standard))
}
