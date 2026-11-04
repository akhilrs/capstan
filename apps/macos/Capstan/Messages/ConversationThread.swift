import CapstanAppCore
import CapstanKit
import SwiftUI

/// One project's conversation with its PM: what the agents sent (host text, untrusted, plain mono blocks with sender and
/// time) and what was sent from here, each with its state: Sending, Sent with the host's delivery, or Not sent with
/// Retry. A question that waits has Reply: the composer then answers that question (`message reply`) instead of sending
/// a new message, and the question's choices fill the answer for the person to send.
struct ConversationThread: View {
    let model: AppModel
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
            case .received(let message): return ThreadTime.parse(message.createdAt) ?? .distantPast
            case .incoming(let item): return ThreadTime.parse(item.since) ?? .distantPast
            case .outgoing(let message): return message.sentAt
            }
        }
    }

    var body: some View {
        let project = model.project(hostId, projectId)
        let pm = project.flatMap(PmBinding.activePm)
        let entries = self.entries(project)
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: Tokens.space[2]) {
                        if entries.isEmpty {
                            Text(verbatim: pm.map { "Nothing said yet. Your message goes to " + $0 + " as plain text." }
                                ?? "This project has no active PM, so nothing can be sent.")
                                .font(.system(size: 13))
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
                    .padding(Tokens.space[3])
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .defaultScrollAnchor(.bottom)
                .onAppear { if let last = entries.last { proxy.scrollTo(last.id, anchor: .bottom) } }
                .onChange(of: entries.count) { _, _ in
                    if let last = entries.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
                }
            }
            Divider()
            composer(pm: pm)
        }
        .onChange(of: project?.messages?[answering?.key ?? ""]?.state) { _, state in
            // The question was answered elsewhere: the composer sends messages again.
            if answering != nil, state != "open" { answering = nil }
        }
        .onChange(of: projectId) { _, _ in
            text = ""
            answering = nil
        }
    }

    private func composer(pm: String?) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if let answering {
                HStack(spacing: Tokens.space[1]) {
                    Image(systemName: "arrowshape.turn.up.left")
                        .foregroundStyle(Tokens.color(.accent))
                        .accessibilityHidden(true)
                    Text(verbatim: "Answering question " + String(answering.sequence) + " from " + answering.senderAgentId)
                        .font(.system(size: 12))
                        .foregroundStyle(Tokens.color(.label2))
                    Spacer(minLength: Tokens.space[1])
                    Button("Cancel") { self.answering = nil }
                        .buttonStyle(.capstanQuiet)
                }
                if !(answering.options ?? []).isEmpty {
                    ChoiceButtons(options: answering.options ?? [], current: text, enabled: !sending) { text = $0 }
                }
            }
            MessageComposer(
                text: $text, enabled: (pm != nil || answering != nil) && !sending,
                placeholder: answering.map { "Answer " + $0.senderAgentId } ?? pm.map { "Message " + $0 }
            ) { body in
                if let question = answering { answer(body, question) } else { send(body) }
            }
        }
        .padding(Tokens.space[2])
    }

    private func entries(_ project: ProjectModel?) -> [Entry] {
        let received = Array((project?.messages ?? [:]).values)
        let known = Set(received.map(\.requestId))
        let older = project?.waiting.values.filter { $0.kind == .message && !known.contains($0.requestId) } ?? []
        let outgoing = model.sent.thread(hostId: hostId, projectId: projectId).map(Entry.outgoing)
        return (received.map(Entry.received) + older.map(Entry.incoming) + outgoing).sorted { $0.at < $1.at }
    }

    /// A message from an agent: its text as written, who sent it and when, and Reply while it is an open question.
    private func received(_ message: StoredMessage) -> some View {
        let time = ThreadTime.parse(message.createdAt)?.formatted(date: .omitted, time: .shortened) ?? ""
        let state: String
        switch (message.messageClass, message.state) {
        case ("action", "open"): state = "Asks you"
        case ("action", _): state = "Answered"
        default: state = "For your information"
        }
        return VStack(alignment: .leading, spacing: Tokens.space[0]) {
            UntrustedTextBlock(
                text: message.body,
                label: message.senderAgentId + " · " + state + (time.isEmpty ? "" : " · " + time) + " · untrusted text")
            if message.isOpenQuestion {
                Button {
                    answering = message
                    text = ""
                } label: {
                    Label { Text("Reply") } icon: { Image(systemName: "arrowshape.turn.up.left") }
                }
                .buttonStyle(.capstanQuiet)
                .disabled(sending)
            }
        }
        .frame(maxWidth: 520, alignment: .leading)
        .id("msg/" + message.key)
    }

    private func incoming(_ item: StoredWaiting, pm: String?) -> some View {
        let detail = model.detail(InboxRowID(hostId: hostId, projectId: projectId, key: item.key))
        let time = ThreadTime.parse(item.since)?.formatted(date: .omitted, time: .shortened) ?? ""
        return UntrustedTextBlock(
            text: detail?.messageBody?.body ?? item.summary,
            label: (pm ?? "PM") + " · needs an answer" + (time.isEmpty ? "" : " · " + time) + " · untrusted text")
            .frame(maxWidth: 520, alignment: .leading)
            .id("in/" + item.key)
    }

    private func outgoing(_ message: SentMessage) -> some View {
        VStack(alignment: .trailing, spacing: Tokens.space[0]) {
            Text(verbatim: message.text)
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.onAccent))
                .padding(.horizontal, Tokens.space[2])
                .padding(.vertical, Tokens.space[1])
                .background(
                    message.state == .sending ? Tokens.color(.accent).opacity(0.7) : Tokens.color(.accent),
                    in: RoundedRectangle(cornerRadius: Tokens.radius.row))
                .textSelection(.enabled)
            stateLine(message)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, Tokens.space[7])
        .id("out/" + message.id.uuidString)
    }

    @ViewBuilder
    private func stateLine(_ message: SentMessage) -> some View {
        let time = message.sentAt.formatted(date: .omitted, time: .shortened)
        switch message.state {
        case .sending:
            Label { Text("Sending") } icon: { ProgressView().controlSize(.mini) }
                .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        case .sent:
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let delivery = MessageStateText.delivery(message, now: context.date)
                let stuck = message.stuck(now: context.date)
                Label {
                    Text(verbatim: delivery.map { $0 + " · " + time } ?? "Sent · on the host " + time)
                } icon: {
                    Image(systemName: stuck ? "exclamationmark.triangle" : (message.delivery.map(MessageStateText.symbol) ?? "checkmark"))
                }
                .font(.system(size: 12))
                .foregroundStyle(stuck ? Tokens.color(.yellow) : Tokens.color(.label2))
            }
        case .notSent(let reason, _):
            HStack(spacing: Tokens.space[1]) {
                Label { Text(verbatim: "Not sent · " + reason) } icon: { Image(systemName: "exclamationmark.triangle") }
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
                Button("Retry") { retry(message) }
                    .buttonStyle(.capstanQuiet)
                    .disabled(sending)
            }
        }
    }

    private func send(_ body: String) {
        guard !sending else { return }
        sending = true
        text = ""
        Task {
            _ = await model.messagePM(body, hostId: hostId, projectId: projectId)
            sending = false
        }
    }

    private func answer(_ body: String, _ question: StoredMessage) {
        guard !sending else { return }
        sending = true
        text = ""
        answering = nil
        Task {
            _ = await model.answerQuestion(body, question: question, hostId: hostId, projectId: projectId)
            sending = false
        }
    }

    private func retry(_ message: SentMessage) {
        guard !sending else { return }
        sending = true
        Task {
            _ = await model.retryMessage(message)
            sending = false
        }
    }
}

enum ThreadTime {
    static func parse(_ text: String) -> Date? {
        let plain = ISO8601DateFormatter()
        if let date = plain.date(from: text) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text)
    }
}
