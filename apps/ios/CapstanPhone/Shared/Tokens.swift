import CapstanAppCore
import SwiftUI
import UIKit

enum TokenRole: CaseIterable {
    case ground, cell, glass, label, label2, sep, fill, fill2, accent, onAccent, red, yellow, green, blue
}

/// The colour roles, type, spacing, radii, motion and haptics of the design (docs/design/iphone-app.md, Tokens). The
/// Mac's roles are unchanged; `ground` and `cell` are added, and `accent` and `label2` have Increase Contrast values.
enum Tokens {
    /// (role, light, dark) as the design's tables write them.
    static let table: [(role: TokenRole, light: String, dark: String)] = [
        (.ground, "#efefec", "#101214"),
        (.cell, "#ffffff", "#1b1d20"),
        (.glass, "rgba(252,252,250,.66)", "rgba(48,51,56,.62)"),
        (.label, "#1c2024", "#eceef0"),
        (.label2, "#545b63", "#a6adb4"),
        (.sep, "rgba(28,32,36,.10)", "rgba(255,255,255,.09)"),
        (.fill, "4% ink", "4% white"),
        (.fill2, "8% ink", "9% white"),
        (.accent, "#8a521c", "#e0a45e"),
        (.onAccent, "#ffffff", "#1a1206"),
        (.red, "#a3221a", "#ffa49c"),
        (.yellow, "#6b5100", "#f0c95a"),
        (.green, "#1d6b35", "#6fd68b"),
        (.blue, "#1d5bb8", "#7fb0ff"),
    ]

    /// Increase Contrast values: (role, light, dark).
    static let highContrast: [(role: TokenRole, light: String, dark: String)] = [
        (.accent, "#6e3f12", "#f0bb7c"),
        (.label2, "#3a4047", "#c9ced3"),
    ]

    /// Concentric radii: grouped sections and option rows, untrusted blocks and fields, small controls.
    static let radius = (largeSheet: 38.0, section: 22.0, block: 12.0, control: 8.0)
    static let space: [CGFloat] = [4, 8, 12, 16, 20, 24, 32]
    static let margin: CGFloat = 16
    /// Buttons are capsules 50 tall; chips 36.
    static let buttonHeight: CGFloat = 50
    static let chipHeight: CGFloat = 36

    /// The role's colour for the current appearance and contrast.
    static func color(_ role: TokenRole) -> Color {
        let normal = table.first { $0.role == role }
        let high = highContrast.first { $0.role == role }
        let light = normal.flatMap { parse($0.light, ink: lightInk) } ?? (0, 0, 0, 1)
        let dark = normal.flatMap { parse($0.dark, ink: (0, 0, 0)) } ?? (1, 1, 1, 1)
        let lightHigh = high.flatMap { parse($0.light, ink: lightInk) } ?? light
        let darkHigh = high.flatMap { parse($0.dark, ink: (0, 0, 0)) } ?? dark
        return Color(uiColor: UIColor { traits in
            let isDark = traits.userInterfaceStyle == .dark
            let isHigh = traits.accessibilityContrast == .high
            let v = isDark ? (isHigh ? darkHigh : dark) : (isHigh ? lightHigh : light)
            return UIColor(red: v.0, green: v.1, blue: v.2, alpha: v.3)
        })
    }

    /// The light label colour: what "N% ink" means in the light column.
    private static var lightInk: (r: Double, g: Double, b: Double) {
        let v = table.first { $0.role == .label }.flatMap { TokenValue.parse($0.light, ink: (0, 0, 0)) }
        return (v?.r ?? 0, v?.g ?? 0, v?.b ?? 0)
    }

    private static func parse(_ s: String, ink: (r: Double, g: Double, b: Double)) -> (Double, Double, Double, Double)? {
        TokenValue.parse(s, ink: ink).map { ($0.r, $0.g, $0.b, $0.a) }
    }
}

/// The design's type scale as Dynamic Type text styles, so every size follows the person's setting.
enum TypeStyle {
    static let largeTitle = Font.largeTitle.weight(.bold)
    static let title2 = Font.title2.weight(.bold)
    static let headline = Font.headline
    static let body = Font.body
    static let subheadline = Font.subheadline
    static let footnote = Font.footnote
    static let mono = Font.system(.body, design: .monospaced)
    static let monoSmall = Font.system(.footnote, design: .monospaced)
    /// Counts and ages use tabular figures.
    static let count = Font.body.monospacedDigit()
}

extension Tone {
    /// Neutral and grey read as secondary text; the others use the role of the same name.
    var color: Color {
        switch self {
        case .neutral, .grey: return Tokens.color(.label2)
        case .green: return Tokens.color(.green)
        case .blue: return Tokens.color(.blue)
        case .yellow: return Tokens.color(.yellow)
        case .red: return Tokens.color(.red)
        case .accent: return Tokens.color(.accent)
        }
    }
}

/// Motion: 150 ms press, 250 ms state change, a 200 ms heartbeat pulse. Reduce Motion drops them.
enum Motion {
    static let press = 0.15
    static let state = 0.25
    static let pulse = 0.2

    static func animation(_ seconds: Double, reduce: Bool) -> Animation? {
        reduce ? nil : .easeInOut(duration: seconds)
    }
}

/// The design's haptics: success when a decision is applied or forwarded; warning on hash_mismatch, needs_terminal
/// and limit_reached; error on an SSH failure; selection on chip and option changes. None on heartbeats.
@MainActor
enum Haptics {
    enum Kind {
        case success, warning, error, selection
    }

    static func play(_ kind: Kind) {
        switch kind {
        case .success: UINotificationFeedbackGenerator().notificationOccurred(.success)
        case .warning: UINotificationFeedbackGenerator().notificationOccurred(.warning)
        case .error: UINotificationFeedbackGenerator().notificationOccurred(.error)
        case .selection: UISelectionFeedbackGenerator().selectionChanged()
        }
    }
}
