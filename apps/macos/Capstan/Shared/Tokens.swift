import AppKit
import CapstanAppCore
import SwiftUI

enum TokenRole: CaseIterable {
    case win, glass, label, label2, sep, fill, fill2, accent, onAccent, red, yellow, green, blue
}

/// The colour roles, radii, spacing and motion of the design (docs/design/mac-app.md, section Tokens).
enum Tokens {
    /// (role, light, dark) in the order of the design's table, written as the table writes them.
    static let table: [(role: TokenRole, light: String, dark: String)] = [
        (.win, "#f6f6f4", "#1b1d20"),
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

    static let radius = (window: 22.0, sheet: 16.0, row: 12.0, block: 10.0, control: 8.0)
    static let space: [CGFloat] = [4, 8, 12, 16, 20, 24, 32, 40]

    /// The role's colour for the current appearance.
    static func color(_ r: TokenRole) -> Color {
        let entry = table.first { $0.role == r }
        let light = entry.flatMap { parse($0.light, ink: lightInk) } ?? (0, 0, 0, 1)
        // Dark values are never parsed with ink: "% white" and the literal forms do not use it.
        let dark = entry.flatMap { parse($0.dark, ink: (0, 0, 0)) } ?? (1, 1, 1, 1)
        return Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            let v = isDark ? dark : light
            return NSColor(srgbRed: v.0, green: v.1, blue: v.2, alpha: v.3)
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

/// Motion of the design: 150 ms for hover and press, 250 ms for a state change. Reduce Motion drops both.
enum Motion {
    static let hover = 0.15
    static let state = 0.25
    static let pulse = 0.2

    static func animation(_ seconds: Double, reduce: Bool) -> Animation? {
        reduce ? nil : .easeInOut(duration: seconds)
    }
}
