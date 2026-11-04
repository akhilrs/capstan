import CapstanAppCore
import CapstanKit
import SwiftUI

/// The toolbar button that starts a message to a PM, in every section (shift-command-N): a popover with a project
/// picker and the message. After sending, Messages to PM opens on the newest conversation, which is this one.
struct MessagePMButton: View {
    let model: AppModel

    @State private var open = false

    var body: some View {
        Button {
            open = true
        } label: {
            Label { Text("Message the PM") } icon: { Image(systemName: "paperplane") }
        }
        .keyboardShortcut("n", modifiers: [.shift, .command])
        .help("Message the PM")
        .popover(isPresented: $open, arrowEdge: .bottom) {
            NewMessagePopover(model: model) { open = false }
        }
    }
}

private struct NewMessagePopover: View {
    let model: AppModel
    let done: () -> Void

    @State private var target: String?
    @State private var text = ""

    var body: some View {
        let choices = model.conversations().filter { $0.pm != nil }
        let chosen = choices.first { $0.id == target } ?? choices.first
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text("Message the PM").font(.system(size: 13, weight: .semibold))
            if choices.isEmpty {
                Text("No project has an active PM, so nothing can be sent.")
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
            } else {
                Picker(selection: Binding(get: { chosen?.id }, set: { target = $0 })) {
                    ForEach(choices) { row in
                        Text(verbatim: row.projectName + " · " + row.hostName + " · " + (row.pm ?? "")).tag(Optional(row.id))
                    }
                } label: {
                    Text("To")
                }
            }
            MessageComposer(
                text: $text, enabled: chosen != nil, placeholder: chosen?.pm.map { "Message " + $0 }
            ) { body in
                guard let chosen else { return }
                text = ""
                model.selection = .messages
                done()
                Task { _ = await model.messagePM(body, hostId: chosen.hostId, projectId: chosen.projectId) }
            }
        }
        .padding(Tokens.space[3])
        .frame(width: 380)
    }
}
