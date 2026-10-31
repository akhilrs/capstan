import CapstanAppCore
import CapstanKit
import SwiftUI

/// The sheet before a destructive choice. Cancel is the default: focus starts on it and it answers Escape on a
/// keyboard. The sheet is opaque because it may show host text. The note, when asked for, is counted in UTF-8 bytes.
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
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text(verbatim: title).font(TypeStyle.title2)
                if let detail {
                    UntrustedTextBlock(text: detail, label: "From the host")
                }
                if let budget {
                    VStack(alignment: .leading, spacing: Tokens.space[0]) {
                        TextField("Note (optional)", text: $note, axis: .vertical)
                            .lineLimit(3...6)
                            .padding(Tokens.space[2])
                            .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
                            .focused($focus, equals: .note)
                        let counter = String(budget.used(note)) + " of " + String(budget.limit) + " bytes"
                        Text(verbatim: counter)
                            .font(TypeStyle.footnote.monospacedDigit())
                            .foregroundStyle(overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                    }
                }
                VStack(spacing: Tokens.space[2]) {
                    Button("Cancel") { dismiss() }
                        .buttonStyle(.capstanQuiet)
                        .keyboardShortcut(.cancelAction)
                        .focused($focus, equals: .cancel)
                    Button(role: .destructive) {
                        onConfirm(budget == nil ? nil : NoteText.normalized(note))
                        dismiss()
                    } label: {
                        Text(verbatim: destructiveLabel)
                    }
                    .buttonStyle(.capstanDestructive)
                    .disabled(overLimit)
                }
            }
            .padding(Tokens.margin)
        }
        .presentationDetents([.medium, .large])
        .presentationBackground(Tokens.color(.cell))
        .onAppear { focus = .cancel }
    }
}

#Preview("ConfirmSheet") {
    Color.clear.sheet(isPresented: .constant(true)) {
        ConfirmSheet(
            title: "Deny this command?",
            detail: UntrustedText(raw: "rm -rf build", verified: true, truncated: false), noteLimit: ProtocolLimits.reasonCap,
            destructiveLabel: "Deny", onConfirm: { _ in })
    }
}
