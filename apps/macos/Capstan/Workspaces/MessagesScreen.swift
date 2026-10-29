import SwiftUI

/// Messages to PM: a conversation per project. The hosts do not carry PM chat yet, so this is the empty state; the
/// messages that wait for an answer stay in Waiting for you.
struct MessagesScreen: View {
    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Image(systemName: "bubble.left").font(.system(size: 26)).foregroundStyle(Tokens.color(.label2))
            Text("Messages to PM arrive with this host's next Capstan update")
                .font(.system(size: 15, weight: .semibold))
            Text("Messages from agents that need your answer are listed under Waiting for you.")
                .font(.system(size: 13))
                .foregroundStyle(Tokens.color(.label2))
        }
        .padding(Tokens.space[5])
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }
}

#Preview("Messages to PM") {
    MessagesScreen().frame(width: 520, height: 320)
}
