import CapstanAppCore
import CapstanKit
import SwiftUI

/// The strip above the waiting groups: the status word of the project, the agent counts and how far they can be
/// trusted. It is built from counts and states only; no agent text is in it.
struct StatusStripView: View {
    let content: OverviewModel
    var onRefresh: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Image(systemName: symbol).foregroundStyle(tone.color)
                Text(verbatim: content.statusWord).font(.system(size: 15, weight: .semibold))
                Text(verbatim: content.countsText)
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: Tokens.space[0])
                Button(action: onRefresh) { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.capstanQuiet)
                    .accessibilityLabel("Refresh")
            }
            if !content.freshnessLine.isEmpty {
                Text(verbatim: content.freshnessLine)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let fact = content.quietFact {
                Text(verbatim: fact)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Project status")
        .accessibilityValue(content.accessibilityValue)
    }

    private var symbol: String {
        switch content.status {
        case .unknown: return "questionmark.circle"
        case .paused: return "pause.circle.fill"
        case .needsAttention: return "exclamationmark.triangle.fill"
        case .moving: return "arrow.triangle.2.circlepath"
        case .quiet: return "moon.zzz.fill"
        }
    }

    private var tone: Tone {
        switch content.status {
        case .unknown, .paused, .quiet: return .grey
        case .needsAttention: return .yellow
        case .moving: return .green
        }
    }
}
