import CapstanAppCore
import CapstanKit
import SwiftUI

/// The message field of Messages to PM: plain text up to the cap with a byte count. Return sends, shift-Return starts a
/// new line. Send is off while the text is empty or over the cap, or while `enabled` is false.
struct MessageComposer: View {
    @Binding var text: String
    let enabled: Bool
    /// "Message pm-1", "Answer pm-1"; nil when nothing can be sent.
    let placeholder: String?
    let onSend: (String) -> Void

    private let budget = TextBudget(limit: SentMessageLog.textCap)

    private func send() {
        guard enabled, ComposerState.check(text, budget: budget) == .ready, let clean = NoteText.normalized(text) else {
            return
        }
        onSend(clean)
    }

    var body: some View {
        let state = ComposerState.check(text, budget: budget)
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .bottom, spacing: Tokens.space[1]) {
                TextField(text: $text, axis: .vertical) {
                    Text(verbatim: placeholder ?? "No active PM")
                }
                .textFieldStyle(.plain)
                .lineLimit(1...6)
                .font(.system(size: 13))
                .padding(.horizontal, Tokens.space[1])
                .padding(.vertical, Tokens.space[0] + 2)
                .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
                .disabled(placeholder == nil)
                .onKeyPress(.return, phases: .down) { press in
                    if press.modifiers.contains(.shift) { return .ignored }
                    send()
                    return .handled
                }
                Button {
                    send()
                } label: {
                    Text("Send")
                }
                .buttonStyle(.capstanPrimary)
                .disabled(!enabled || state != .ready)
            }
            HStack {
                Text(verbatim: String(budget.used(text)) + " of " + String(budget.limit) + " bytes")
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(state == .overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                Spacer()
                Text("Return sends · shift-Return adds a line")
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
            }
        }
    }
}
