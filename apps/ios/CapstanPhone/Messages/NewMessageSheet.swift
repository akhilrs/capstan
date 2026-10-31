import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// New message (board 11), at the large detent: the project and its PM, the message (plain text, up to 500 bytes),
/// Send. A sheet with typed text asks before it is discarded. After sending, the conversation opens.
struct NewMessageSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var target: String?
    @State private var text = ""
    @State private var askDiscard = false
    @FocusState private var fieldFocused: Bool

    private let budget = TextBudget(limit: SentMessageLog.textCap)

    var body: some View {
        let choices = model.conversations().filter { $0.pm != nil }
        let chosen = choices.first { $0.id == target } ?? choices.first
        let state = ComposerState.check(text, budget: budget)
        NavigationStack {
            Form {
                Section {
                    Picker(selection: Binding(get: { chosen?.id }, set: { target = $0 })) {
                        ForEach(choices) { row in
                            Text(verbatim: row.projectName + " · " + (row.pm ?? "")).tag(Optional(row.id))
                        }
                    } label: {
                        Text("To")
                    }
                }
                Section {
                    TextField(text: $text, axis: .vertical) { Text("Your message") }
                        .lineLimit(4...10)
                        .focused($fieldFocused)
                } header: {
                    Text("Message")
                } footer: {
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                            .monospacedDigit()
                            .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                        Text("Plain text. The PM reads it on the host; it is never run as a command.")
                    }
                }
            }
            .navigationTitle("New message")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { if text.isEmpty { dismiss() } else { askDiscard = true } }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") { send(chosen) }.disabled(chosen == nil || state != .ready)
                }
            }
            .confirmationDialog("Discard your message?", isPresented: $askDiscard, titleVisibility: .visible) {
                Button("Discard", role: .destructive) { dismiss() }
                Button("Keep editing", role: .cancel) {}
            }
        }
        .presentationDetents([.large])
        .interactiveDismissDisabled(!text.isEmpty)
        .onAppear { fieldFocused = true }
    }

    private func send(_ row: Conversation?) {
        guard let row, let body = NoteText.normalized(text) else { return }
        let host = row.hostId
        let project = row.projectId
        model.navigation.show(.messages, [.conversation(hostId: host, projectId: project)])
        dismiss()
        Task {
            let outcome = await model.messagePM(body, hostId: host, projectId: project)
            if case .applied = outcome { Haptics.play(.success) } else if case .refused = outcome { Haptics.play(.warning) }
        }
    }
}
