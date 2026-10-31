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

    /// Writes the glance, updates the Live Activity and the badge, and posts the notifications for items not seen
    /// before (`RefreshPlanner`); called after the hosts' models or connections change, here and in a background
    /// refresh alike, so each item is announced once.
    func publishGlance() {
        let glance = makeGlance()
        if let directory { try? GlanceStore(directory: directory).write(glance) }
        liveActivity.apply(glance, allowed: settings.liveActivityAllowed)
        notifier.setBadge(glance.totalWaiting)
        announceNew()
    }

    /// Posts what waits and was not seen before. Only hosts whose snapshot is complete take part, so a half-read
    /// host is not taken for one where items vanished, and its remembered items are kept for the next pass.
    private func announceNew() {
        guard let directory else { return }
        let complete = hosts.compactMap { hostModels[$0.id] }.filter(RefreshPlanner.isComplete)
        guard !complete.isEmpty else { return }
        let store = SeenStore(directory: directory)
        let previous = store.read()
        let pass = RefreshPlanner.pass(models: complete, seen: previous, settings: settings) { name in
            FileManager.default.fileExists(atPath: PhoneSounds.userDirectory.appendingPathComponent(name).path)
        }
        let readPrefixes = complete.map { $0.hostId.uuidString + "/" }
        let kept = (previous ?? []).filter { key in !readPrefixes.contains { key.hasPrefix($0) } }
        let next = pass.seen.union(kept)
        if next != previous { try? store.write(next) }
        if !pass.notifications.isEmpty { notifier.post(pass.notifications) }
    }
}
