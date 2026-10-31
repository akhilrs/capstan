import SwiftUI

/// The light confirmation before an answer goes to the PM (board 04): what is sent, who confirms it, and that nothing is
/// answered until then. A small detent; Cancel is the default.
struct ForwardConfirmSheet: View {
    let title: String
    let text: String
    let confirmLabel: String
    let onConfirm: () -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize
    @FocusState private var cancelFocused: Bool

    var body: some View {
        VStack(spacing: Tokens.space[3]) {
            Text(verbatim: title)
                .font(TypeStyle.headline)
                .multilineTextAlignment(.center)
            Text(verbatim: text)
                .font(TypeStyle.subheadline)
                .foregroundStyle(Tokens.color(.label2))
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if typeSize.isAccessibilitySize {
                VStack(spacing: Tokens.space[2]) { buttons }
            } else {
                HStack(spacing: Tokens.space[2]) { buttons }
            }
        }
        .padding(Tokens.margin)
        .padding(.top, Tokens.space[2])
        .presentationDetents(typeSize.isAccessibilitySize ? [.medium, .large] : [.height(260), .medium])
        .presentationDragIndicator(.visible)
        .onAppear { cancelFocused = true }
    }

    @ViewBuilder
    private var buttons: some View {
        Button("Cancel") { dismiss() }
            .buttonStyle(.capstanQuiet)
            .keyboardShortcut(.cancelAction)
            .focused($cancelFocused)
        Button {
            onConfirm()
            dismiss()
        } label: {
            Text(verbatim: confirmLabel)
        }
        .buttonStyle(.capstanPrimary)
    }
}

#Preview("ForwardConfirmSheet") {
    Color.clear.sheet(isPresented: .constant(true)) {
        ForwardConfirmSheet(
            title: "Send option 1 to the PM?",
            text: "The PM asks you to confirm, then answers the prompt in dev-3's terminal. Nothing is answered until then.",
            confirmLabel: "Send to PM", onConfirm: {})
    }
}
