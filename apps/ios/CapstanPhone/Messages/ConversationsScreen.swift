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

extension AppModel {
    /// The conversations: every ready project with an active PM, a message from an agent or a sent message, newest
    /// first.
    func conversations() -> [Conversation] {
        Conversations.make(hosts: hosts, hostModels: hostModels, sent: messages.log)
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

#Preview("Messages") {
    NavigationStack { ConversationsScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
}
