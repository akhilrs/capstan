import SwiftUI

/// The short note at the top after a forward (board 05): "Sent to the PM" on the floating layer, gone after a few
/// seconds and announced to VoiceOver. It names the PM only; no agent text.
struct OutcomeToast: ViewModifier {
    @Binding var shown: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.overlay(alignment: .top) {
            if shown {
                HStack(spacing: Tokens.space[2]) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Tokens.color(.green))
                        .accessibilityHidden(true)
                    Text("Sent to the PM").font(TypeStyle.subheadline.weight(.semibold))
                }
                .padding(.horizontal, Tokens.space[3])
                .frame(minHeight: 44)
                .glassEffect(.regular, in: Capsule())
                .padding(.top, Tokens.space[1])
                .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
                .task {
                    AccessibilityNotification.Announcement("Sent to the PM").post()
                    try? await Task.sleep(for: .seconds(3))
                    withAnimation(Motion.animation(Motion.state, reduce: reduceMotion)) { shown = false }
                }
                .onTapGesture { shown = false }
            }
        }
        .animation(Motion.animation(Motion.state, reduce: reduceMotion), value: shown)
    }
}

extension View {
    func outcomeToast(_ shown: Binding<Bool>) -> some View {
        modifier(OutcomeToast(shown: shown))
    }
}

#Preview("OutcomeToast") {
    @Previewable @State var shown = true
    Color.clear
        .outcomeToast($shown)
        .background(Tokens.color(.ground))
}
