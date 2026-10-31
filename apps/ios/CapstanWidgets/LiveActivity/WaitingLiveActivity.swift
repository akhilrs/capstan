import ActivityKit
import CapstanPhoneCore
import SwiftUI
import WidgetKit

/// The "N waiting" Live Activity (board 15): the count, the kinds, which hosts are live and when it was updated.
/// Counts and kinds only, never anything an agent wrote. Tapping it opens the app.
struct WaitingLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: WaitingActivityAttributes.self) { context in
            LockScreenView(state: context.state, stale: context.isStale)
                .activityBackgroundTint(nil)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Label("Capstan", systemImage: "tray")
                        .font(.headline)
                        .foregroundStyle(.white)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(verbatim: String(context.state.total))
                        .font(.title.weight(.bold).monospacedDigit())
                        .foregroundStyle(WidgetColors.accent)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(verbatim: context.state.kindsLine)
                            .font(.subheadline)
                            .foregroundStyle(.white)
                        Text(verbatim: activityFooter(context.state, stale: context.isStale))
                            .font(.caption)
                            .foregroundStyle(.white.opacity(0.7))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } compactLeading: {
                Image(systemName: "tray").foregroundStyle(WidgetColors.accent)
            } compactTrailing: {
                Text(verbatim: String(context.state.total))
                    .font(.headline.monospacedDigit())
                    .foregroundStyle(WidgetColors.accent)
            } minimal: {
                Text(verbatim: String(context.state.total))
                    .font(.headline.monospacedDigit())
                    .foregroundStyle(WidgetColors.accent)
            }
        }
    }
}

func activityFooter(_ state: WaitingActivityState, stale: Bool) -> String {
    let time = state.asOf.formatted(date: .omitted, time: .shortened)
    return stale ? "as of " + time : state.hostsLine + " · updated " + time
}

/// The Lock Screen banner: the count, "waiting for you", the kinds, a bar in the accent, and the footer.
struct LockScreenView: View {
    let state: WaitingActivityState
    let stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: "tray").foregroundStyle(.secondary)
                Text(verbatim: String(state.total))
                    .font(.largeTitle.weight(.bold).monospacedDigit())
                VStack(alignment: .leading, spacing: 2) {
                    Text("waiting for you").font(.headline)
                    Text(verbatim: state.shortLine()).font(.subheadline).foregroundStyle(.secondary)
                }
            }
            Capsule()
                .fill(stale ? Color.secondary : WidgetColors.accent)
                .frame(height: 4)
                .accessibilityHidden(true)
            Text(verbatim: activityFooter(state, stale: stale)).font(.caption).foregroundStyle(.secondary)
        }
        .padding(16)
        .accessibilityElement(children: .combine)
    }
}

/// The design's accent (rope amber) for the extension, which does not see the app's tokens.
enum WidgetColors {
    static let accent = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0xE0 / 255, green: 0xA4 / 255, blue: 0x5E / 255, alpha: 1)
            : UIColor(red: 0x8A / 255, green: 0x52 / 255, blue: 0x1C / 255, alpha: 1)
    })
}

#Preview("Lock Screen", as: .content, using: WaitingActivityAttributes()) {
    WaitingLiveActivity()
} contentStates: {
    WaitingActivityState(GlanceSnapshot(
        asOf: Date(), kindCounts: ["op": 2, "prompt": 2, "design": 1, "message": 1, "integration": 1], projects: [],
        hostsReachable: 1, hostsTotal: 1, pollMs: 2000))
}
