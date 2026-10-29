import SwiftUI

/// The buttons of the design: a capsule. Primary is the accent with on-accent text, quiet is a tonal fill, and
/// destructive is an outline in red.
struct CapstanButtonStyle: ButtonStyle {
    enum Kind { case primary, quiet, destructive }

    let kind: Kind
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: kind == .primary ? .semibold : .medium))
            .foregroundStyle(foreground)
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            .background(background, in: Capsule())
            .overlay(Capsule().strokeBorder(kind == .destructive ? Tokens.color(.red) : Color.clear, lineWidth: 1))
            .opacity(enabled ? (configuration.isPressed ? 0.8 : 1) : 0.45)
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
