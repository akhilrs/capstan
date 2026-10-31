import ActivityKit
import CapstanPhoneCore
import Foundation

/// Starts, updates and ends the "N waiting" Live Activity from the glance snapshot (board 15). It starts when the
/// first item waits and the person allowed it in Settings, and ends when nothing waits or they switch it off. No push
/// token is ever requested (`pushType: nil`, D8): the app updates it while it runs, background refresh later.
@MainActor
final class LiveActivityController {
    /// Counts and hosts as last sent, and when; the time alone is refreshed at most every `refreshSeconds`.
    private var last: (key: String, at: Date)?
    static let refreshSeconds: TimeInterval = 30
    /// The activity says "as of" once no update came for this long (the app stopped following, or was suspended).
    static let staleAfter: TimeInterval = 90

    /// The snapshot changed (or the setting did): start, update or end the activity.
    func apply(_ snapshot: GlanceSnapshot, allowed: Bool, now: Date = Date()) {
        let state = WaitingActivityState(snapshot)
        let running = Activity<WaitingActivityAttributes>.activities
        guard allowed, ActivityAuthorizationInfo().areActivitiesEnabled, state.total > 0 else {
            last = nil
            if !running.isEmpty { endRunning() }
            return
        }
        let key = String(state.total) + "|" + state.kindsLine + "|" + state.hostsLine
        if let last, last.key == key, now.timeIntervalSince(last.at) < Self.refreshSeconds, !running.isEmpty { return }
        last = (key, now)
        let content = ActivityContent(state: state, staleDate: snapshot.asOf.addingTimeInterval(Self.staleAfter))
        if !running.isEmpty {
            // Activity is not Sendable: the detached task reads the activities itself, so none crosses an actor.
            Task.detached {
                let all = Activity<WaitingActivityAttributes>.activities
                await all.first?.update(content)
                for extra in all.dropFirst() { await extra.end(nil, dismissalPolicy: .immediate) }
            }
        } else {
            _ = try? Activity.request(attributes: WaitingActivityAttributes(), content: content, pushType: nil)
        }
    }

    /// Ends every activity at once (the app is told nothing waits, or the switch went off).
    func endAll() {
        last = nil
        endRunning()
    }

    private func endRunning() {
        Task.detached {
            for activity in Activity<WaitingActivityAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
        }
    }
}
