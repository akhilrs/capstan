import CapstanAppCore
import CapstanKit
import SwiftUI

/// A text box with a byte counter; the send button is on only while the text fits the cap.
struct ReplyComposer: View {
    let title: String
    let sendLabel: String
    let enabled: Bool
    /// The choices of a question; a click fills the box, and the person still sends.
    var options: [String] = []
    var limit = ProtocolLimits.reasonCap
    let onSend: (String) -> Void

    @State private var text = ""
    private var budget: TextBudget { TextBudget(limit: limit) }

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Tokens.color(.label2))
            if !options.isEmpty {
                ChoiceButtons(options: options, current: text, enabled: enabled) { text = $0 }
            }
            TextEditor(text: $text)
                .font(.system(size: 13))
                .frame(minHeight: 72, maxHeight: 140)
                .scrollContentBackground(.hidden)
                .padding(Tokens.space[0])
                .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
            HStack {
                Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                Spacer()
                Button {
                    if let clean = NoteText.normalized(text) { onSend(clean) }
                } label: {
                    Text(verbatim: sendLabel)
                }
                .buttonStyle(.capstanPrimary)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(!enabled || state != .ready)
            }
        }
    }
}
