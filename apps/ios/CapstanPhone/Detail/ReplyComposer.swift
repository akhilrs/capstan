import CapstanAppCore
import CapstanKit
import SwiftUI

/// The reply bar of a message from the PM (board 03): a text field and a send button that is off while the text is
/// empty or over the cap. The text is sent as plain text; the counter shows once the text nears the cap.
struct ReplyBar: View {
    @Binding var text: String
    /// The choices of the question, shown as buttons that fill the reply; empty without.
    var options: [String] = []
    let enabled: Bool
    let onSend: (String) -> Void

    private let budget = TextBudget(limit: ProtocolLimits.reasonCap)

    private func send() {
        guard enabled, ComposerState.check(text, budget: budget) == .ready, let clean = NoteText.normalized(text) else {
            return
        }
        onSend(clean)
        text = ""
    }

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if !options.isEmpty {
                ChoiceButtons(options: options, current: text, enabled: enabled) { text = $0 }
                    .padding(.bottom, Tokens.space[1])
            }
            if budget.used(text) * 5 >= budget.limit * 4 {
                Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                    .font(TypeStyle.footnote.monospacedDigit())
                    .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
            }
            HStack(alignment: .bottom, spacing: Tokens.space[2]) {
                TextField(text: $text, axis: .vertical) {
                    if options.isEmpty { Text("Your reply") } else { Text("Pick a choice or write your answer") }
                }
                    .lineLimit(1...5)
                    .sendOnReturn($text, send: send)
                    .font(TypeStyle.body)
                    .padding(.horizontal, Tokens.space[3])
                    .padding(.vertical, Tokens.space[2])
                    .frame(minHeight: 44)
                    .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: 22))
                    .disabled(!enabled)
                Button {
                    send()
                } label: {
                    Image(systemName: "arrow.up")
                        .font(TypeStyle.headline)
                        .foregroundStyle(state == .ready && enabled ? Tokens.color(.onAccent) : Tokens.color(.label2))
                        .frame(width: 44, height: 44)
                        .background(state == .ready && enabled ? Tokens.color(.accent) : Tokens.color(.fill2), in: Circle())
                }
                .accessibilityLabel("Send reply")
                .disabled(!enabled || state != .ready)
            }
        }
    }
}

/// "Request changes…" for a design (board 03): what should change, as plain text up to the cap. A sheet with typed
/// text asks before it is discarded.
struct ChangesSheet: View {
    let onSend: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var askDiscard = false
    @FocusState private var fieldFocused: Bool

    private let budget = TextBudget(limit: ProtocolLimits.reasonCap)

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    Text("What should change?").font(TypeStyle.footnote.weight(.semibold))
                    TextField(text: $text, axis: .vertical) { Text("Describe the changes") }
                        .lineLimit(4...10)
                        .padding(Tokens.space[2])
                        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
                        .focused($fieldFocused)
                    Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                        .font(TypeStyle.footnote.monospacedDigit())
                        .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                    Text("The designer gets your words as plain text. Approve stays possible later.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                }
                .padding(Tokens.margin)
            }
            .navigationTitle("Request changes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        if text.isEmpty { dismiss() } else { askDiscard = true }
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") {
                        if let clean = NoteText.normalized(text) {
                            onSend(clean)
                            dismiss()
                        }
                    }
                    .disabled(state != .ready)
                }
            }
            .confirmationDialog("Discard your text?", isPresented: $askDiscard, titleVisibility: .visible) {
                Button("Discard", role: .destructive) { dismiss() }
                Button("Keep editing", role: .cancel) {}
            }
        }
        .presentationDetents([.medium, .large])
        .interactiveDismissDisabled(!text.isEmpty)
        .onAppear { fieldFocused = true }
    }
}

#Preview("ReplyBar") {
    @Previewable @State var text = ""
    VStack {
        Spacer()
        ReplyBar(text: $text, enabled: true, onSend: { _ in }).padding()
    }
    .background(Tokens.color(.ground))
}

#Preview("ChangesSheet") {
    Color.clear.sheet(isPresented: .constant(true)) { ChangesSheet(onSend: { _ in }) }
}
