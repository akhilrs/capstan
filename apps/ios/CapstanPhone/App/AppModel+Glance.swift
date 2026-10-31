import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation

/// The glance: the agent-text-free snapshot of what waits (package `snapshot-store`), kept in Application Support and
/// handed to the Live Activity. Widgets read it once an App Group can be signed (a paid team; see R13).
extension AppModel {
    func makeGlance(now: Date = Date()) -> GlanceSnapshot {
        let live = hosts.filter { host in
            if case .live? = connection[host.id] { return true }
            return false
        }.count
        return GlanceSnapshot.make(
            hosts: hosts.compactMap { hostModels[$0.id] }, hostsReachable: live, hostsTotal: hosts.count,
            pollMs: settings.pollMs, asOf: now)
    }

    /// Writes the glance and updates the Live Activity; called after the hosts' models or connections change.
    func publishGlance() {
        let glance = makeGlance()
        if let directory { try? GlanceStore(directory: directory).write(glance) }
        liveActivity.apply(glance, allowed: settings.liveActivityAllowed)
    }
}
