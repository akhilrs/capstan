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
    /// The header of the host text block.
    let detailLabel: String
    /// The label above the note field.
    let noteLabel: String
    /// A line under the note that says what the choice does.
    let footnote: String?
    let onConfirm: (String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var note = ""
    @FocusState private var focus: Field?

    private enum Field { case cancel, note }

    init(
        title: String, detail: UntrustedText?, noteLimit: Int?, destructiveLabel: String,
        detailLabel: String = "From the host", noteLabel: String = "Note (optional)", footnote: String? = nil,
        onConfirm: @escaping (String?) -> Void
    ) {
        self.detailLabel = detailLabel
        self.noteLabel = noteLabel
        self.footnote = footnote
        self.title = title
        self.detail = detail
        self.noteLimit = noteLimit
        self.destructiveLabel = destructiveLabel
        self.onConfirm = onConfirm
    }

    private var budget: TextBudget? { noteLimit.map { TextBudget(limit: $0) } }
    private var overLimit: Bool { budget.map { !$0.allows(note) } ?? false }

    @ViewBuilder
    private var buttons: some View {
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

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text(verbatim: title).font(TypeStyle.title2)
                if let detail {
                    UntrustedTextBlock(text: detail, label: detailLabel)
                }
                if let budget {
                    VStack(alignment: .leading, spacing: Tokens.space[0]) {
                        Text(verbatim: noteLabel).font(TypeStyle.footnote.weight(.semibold))
                        TextField(text: $note, axis: .vertical) { Text(verbatim: noteLabel) }
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
                if let footnote {
                    Text(verbatim: footnote).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
                // Side by side, Cancel first; stacked at accessibility sizes.
                if typeSize.isAccessibilitySize {
                    VStack(spacing: Tokens.space[2]) { buttons }
                } else {
                    HStack(spacing: Tokens.space[2]) { buttons }
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
