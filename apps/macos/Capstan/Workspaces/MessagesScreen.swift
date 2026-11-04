import CapstanAppCore
import CapstanKit
import SwiftUI

/// Messages to PM: a list of conversations, one per project with a PM, and the thread of the one in view. Which
/// conversation is in view is kept here, not in the sidebar selection; with none picked, the newest shows.
struct MessagesScreen: View {
    let model: AppModel

    @State private var picked: String?

    var body: some View {
        let rows = model.conversations()
        if rows.isEmpty {
            empty
        } else {
            let current = rows.first { $0.id == picked } ?? rows[0]
            HStack(spacing: 0) {
                ConversationList(rows: rows, selected: current.id) { picked = $0 }
                    .frame(width: 280)
                Divider()
                ConversationThread(model: model, hostId: current.hostId, projectId: current.projectId)
                    .id(current.id)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Image(systemName: "bubble.left").font(.system(size: 26)).foregroundStyle(Tokens.color(.label2))
            Text("No conversations")
                .font(.system(size: 15, weight: .semibold))
            Text("A project with an active PM appears here. Messages from agents that need your answer are also listed under Waiting for you.")
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.label2))
        }
        .padding(Tokens.space[5])
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }
}

extension AppModel {
    /// The conversations: every ready project with an active PM, a message from an agent or a sent message, newest first.
    func conversations() -> [Conversation] {
        Conversations.make(hosts: hosts, hostModels: hostModels, sent: sent)
    }
}

#Preview("Messages to PM") {
    MessagesScreen(model: AppModel(preview: PreviewData.standard)).frame(width: 760, height: 460)
}
