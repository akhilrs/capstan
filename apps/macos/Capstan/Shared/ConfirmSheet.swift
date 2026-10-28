import CapstanAppCore
import CapstanKit
import SwiftUI

/// The sheet before a destructive choice. Focus starts on Cancel; the note, when asked for, is counted in UTF-8 bytes.
struct ConfirmSheet: View {
    let title: String
    let detail: UntrustedText?
    let noteLimit: Int?
    let destructiveLabel: String
    let onConfirm: (String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var note = ""
    @FocusState private var focus: Field?

    private enum Field { case cancel, note }

    init(
        title: String, detail: UntrustedText?, noteLimit: Int?, destructiveLabel: String,
        onConfirm: @escaping (String?) -> Void
    ) {
        self.title = title
        self.detail = detail
        self.noteLimit = noteLimit
        self.destructiveLabel = destructiveLabel
        self.onConfirm = onConfirm
    }

    private var budget: TextBudget? { noteLimit.map { TextBudget(limit: $0) } }
    private var overLimit: Bool { budget.map { !$0.allows(note) } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: title).font(.system(size: 15, weight: .semibold))
            if let detail {
                UntrustedTextBlock(text: detail, label: "From the host")
            }
            if let budget {
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    TextField("Note (optional)", text: $note, axis: .vertical)
                        .lineLimit(3...6)
                        .textFieldStyle(.roundedBorder)
                        .focused($focus, equals: .note)
                    let counter = String(budget.used(note)) + " of " + String(budget.limit) + " bytes"
                    Text(verbatim: counter)
                        .font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                }
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .focused($focus, equals: .cancel)
                Button(role: .destructive) {
                    onConfirm(budget == nil ? nil : NoteText.normalized(note))
                    dismiss()
                } label: {
                    Text(verbatim: destructiveLabel).foregroundStyle(Tokens.color(.red))
                }
                .disabled(overLimit)
            }
        }
        .padding(Tokens.space[4])
        .frame(width: 420)
        .onAppear { focus = .cancel }
    }
}

#Preview("ConfirmSheet") {
    ConfirmSheet(
        title: "Deny this command?",
        detail: UntrustedText(raw: "rm -rf build", verified: true, truncated: false), noteLimit: nil,
        destructiveLabel: "Deny", onConfirm: { _ in })
}

#Preview("ConfirmSheet with note") {
    ConfirmSheet(
        title: "Pause the run?", detail: nil, noteLimit: ProtocolLimits.reasonCap, destructiveLabel: "Pause",
        onConfirm: { _ in })
}
