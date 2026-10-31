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
    /// A sample from Device checks is showing: the app's own updates leave it alone until it ends.
    private(set) var sampling = false
    /// What the last call did, for Device checks.
    private(set) var lastResult = "Nothing yet"

    /// Shows a sample whatever Settings says, until `endAll()` or `resumeReal()`.
    func showSample(_ snapshot: GlanceSnapshot) {
        sampling = false
        apply(snapshot, allowed: true)
        sampling = true
    }

    /// Back to the real counts.
    func resumeReal() {
        sampling = false
        last = nil
    }

    /// The snapshot changed (or the setting did): start, update or end the activity.
    func apply(_ snapshot: GlanceSnapshot, allowed: Bool, now: Date = Date()) {
        guard !sampling else { return }
        let state = WaitingActivityState(snapshot)
        let running = Activity<WaitingActivityAttributes>.activities
        let enabled = ActivityAuthorizationInfo().areActivitiesEnabled
        guard allowed, enabled, state.total > 0 else {
            last = nil
            if !running.isEmpty { endRunning() }
            lastResult = !enabled ? "Live Activities are off for Capstan in iOS Settings"
                : !allowed ? "Off in Settings → Live Activity when items wait" : "Nothing waits"
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
            lastResult = "Updated: " + String(state.total) + " waiting"
        } else {
            do {
                _ = try Activity.request(attributes: WaitingActivityAttributes(), content: content, pushType: nil)
                lastResult = "Started: " + String(state.total) + " waiting"
            } catch {
                last = nil
                lastResult = "iOS refused to start it: " + String(describing: error)
            }
        }
    }

    /// Ends every activity at once (the app is told nothing waits, or the switch went off).
    func endAll() {
        last = nil
        sampling = false
        lastResult = "Ended"
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
