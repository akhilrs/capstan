import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// The Messages tab (board 11): one conversation per project's PM, the newest first, with what was said last and how
/// many PM messages wait for an answer. The compose button starts a new message with a project picker.
struct ConversationsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var composing = false

    var body: some View {
        let rows = model.conversations()
        List {
            Section {
                Text("One conversation per project's PM")
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                    .listRowBackground(Color.clear)
            }
            if rows.isEmpty {
                Section {
                    ContentUnavailableView(
                        "No conversations", systemImage: "bubble.left",
                        description: Text("A project with an active PM appears here."))
                }
                .listRowBackground(Color.clear)
            } else {
                Section {
                    ForEach(rows) { row in
                        NavigationLink(value: PhoneRoute.conversation(hostId: row.hostId, projectId: row.projectId)) {
                            ConversationRow(row: row)
                        }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Tokens.color(.ground))
        .navigationTitle("Messages")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { composing = true } label: { Label("New message", systemImage: "square.and.pencil") }
                    .disabled(rows.allSatisfy { $0.pm == nil })
            }
        }
        .sheet(isPresented: $composing) { NewMessageSheet() }
    }
}

/// One project's conversation in the list.
struct Conversation: Identifiable {
    let hostId: UUID
    let projectId: String
    let projectName: String
    let hostName: String
    /// The active PM; nil when the project has none (nothing can be sent).
    let pm: String?
    /// PM messages waiting for an answer.
    let waiting: Int
    /// The newest line: "pm-1: …" or "You: … · Sent".
    let last: String
    let lastAt: Date?
    var id: String { hostId.uuidString + "/" + projectId }
}

extension AppModel {
    /// The conversations: every ready project with an active PM, a waiting PM message or a sent message, newest first.
    func conversations() -> [Conversation] {
        var rows: [Conversation] = []
        for host in hosts {
            for project in (hostModels[host.id]?.projects.values).map(Array.init) ?? [] where project.availability == .ready {
                let pm = PmBinding.activePm(project)
                let incoming = project.waiting.values.filter { $0.kind == .message }
                let sent = messages.thread(hostId: host.id, projectId: project.projectId)
                guard pm != nil || !incoming.isEmpty || !sent.isEmpty else { continue }
                let newestIn = incoming.max { $0.since < $1.since }
                let inAt = newestIn.flatMap { DetailTime.parse($0.since) }
                let newestOut = sent.last
                let last: String
                let lastAt: Date?
                if let newestOut, inAt.map({ newestOut.sentAt >= $0 }) ?? true {
                    last = "You: " + newestOut.text + " · " + MessageStateText.short(newestOut.state)
                    lastAt = newestOut.sentAt
                } else if let newestIn {
                    let text = newestIn.summary.verified
                        ? TextBudget.displayPrefix(newestIn.summary.displayPlain, maxBytes: 200).text
                        : "A message that failed its integrity check"
                    last = (pm ?? "PM") + ": " + text
                    lastAt = inAt
                } else {
                    last = "No messages yet"
                    lastAt = nil
                }
                rows.append(Conversation(
                    hostId: host.id, projectId: project.projectId,
                    projectName: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text, hostName: host.displayName,
                    pm: pm, waiting: incoming.count, last: last, lastAt: lastAt))
            }
        }
        return rows.sorted { ($0.lastAt ?? .distantPast) > ($1.lastAt ?? .distantPast) }
    }
}

private struct ConversationRow: View {
    let row: Conversation

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Text(verbatim: row.projectName).font(TypeStyle.headline)
                Text(verbatim: "· " + row.hostName).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                Spacer(minLength: Tokens.space[1])
                if row.waiting > 0 {
                    Text(verbatim: String(row.waiting))
                        .font(TypeStyle.footnote.weight(.semibold).monospacedDigit())
                        .padding(.horizontal, Tokens.space[1])
                        .background(Tokens.color(.accent).opacity(0.18), in: Capsule())
                        .accessibilityLabel(
                            Text(verbatim: String(row.waiting) + " waiting"))
                }
                if let at = row.lastAt {
                    Text(verbatim: AgeText.short(since: ISO8601DateFormatter().string(from: at), now: Date()))
                        .font(TypeStyle.subheadline.monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                }
            }
            Text(verbatim: row.last)
                .font(TypeStyle.subheadline)
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(2)
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }
}

/// The words of a sent message's state.
enum MessageStateText {
    static func short(_ state: SentMessage.State) -> String {
        switch state {
        case .sending: return "Sending"
        case .sent: return "Sent"
        case .notSent: return "Not sent"
        }
    }
}

#Preview("Messages") {
    NavigationStack { ConversationsScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
}
