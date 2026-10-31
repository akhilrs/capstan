import SwiftUI

/// The buttons of the design: capsules 50 tall. Primary is the one accent with on-accent text, quiet is a tonal fill,
/// destructive is an outline in red (state colours never fill a primary action).
struct CapstanButtonStyle: ButtonStyle {
    enum Kind { case primary, quiet, destructive }

    let kind: Kind
    @Environment(\.isEnabled) private var enabled
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(TypeStyle.headline.weight(kind == .primary ? .semibold : .medium))
            .foregroundStyle(foreground)
            .frame(maxWidth: .infinity, minHeight: Tokens.buttonHeight)
            .padding(.horizontal, Tokens.space[3])
            .background(background, in: Capsule())
            .overlay(Capsule().strokeBorder(kind == .destructive ? Tokens.color(.red) : Color.clear, lineWidth: 1))
            .opacity(enabled ? (configuration.isPressed ? 0.8 : 1) : 0.45)
            .animation(Motion.animation(Motion.press, reduce: reduceMotion), value: configuration.isPressed)
            .contentShape(Capsule())
    }

    private var foreground: Color {
        switch kind {
        case .primary: return Tokens.color(.onAccent)
        case .quiet: return Tokens.color(.label)
        case .destructive: return Tokens.color(.red)
        }
    }

    private var background: Color {
        switch kind {
        case .primary: return Tokens.color(.accent)
        case .quiet: return Tokens.color(.fill2)
        case .destructive: return Color.clear
        }
    }
}

extension ButtonStyle where Self == CapstanButtonStyle {
    static var capstanPrimary: CapstanButtonStyle { CapstanButtonStyle(kind: .primary) }
    static var capstanQuiet: CapstanButtonStyle { CapstanButtonStyle(kind: .quiet) }
    static var capstanDestructive: CapstanButtonStyle { CapstanButtonStyle(kind: .destructive) }
}

#Preview("Buttons") {
    VStack(spacing: Tokens.space[2]) {
        Button("Forward to PM") {}.buttonStyle(.capstanPrimary)
        Button("Copy") {}.buttonStyle(.capstanQuiet)
        Button("Deny") {}.buttonStyle(.capstanDestructive)
        Button("Disabled") {}.buttonStyle(.capstanPrimary).disabled(true)
    }
    .padding()
}
