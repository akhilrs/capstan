import SwiftUI
import CapstanAppCore

/// A state as a word and a glyph; red is outlined, the other tones are tinted.
struct StatePill: View {
    let word: String
    let symbol: String?
    let tone: Tone

    var body: some View {
        HStack(spacing: Tokens.space[0]) {
            if let symbol { Image(systemName: symbol).imageScale(.small) }
            Text(verbatim: word).font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(tone.color)
        .padding(.horizontal, Tokens.space[1])
        .padding(.vertical, Tokens.space[0])
        .background(tone == .red ? Color.clear : tone.color.opacity(0.14), in: Capsule())
        .overlay(Capsule().strokeBorder(tone.color, lineWidth: tone == .red ? 1 : 0))
        .accessibilityElement(children: .combine)
    }
}

#Preview("StatePill") {
    VStack(alignment: .leading) {
        StatePill(word: "Applied", symbol: "checkmark.circle.fill", tone: .green)
        StatePill(word: "Forwarded", symbol: "arrow.right.circle.fill", tone: .blue)
        StatePill(word: "Stale", symbol: "clock.fill", tone: .yellow)
        StatePill(word: "Lost", symbol: "xmark.octagon.fill", tone: .red)
        StatePill(word: "Idle", symbol: "moon.zzz.fill", tone: .grey)
        StatePill(word: "Sending", symbol: "paperplane.fill", tone: .neutral)
    }
    .padding()
}
