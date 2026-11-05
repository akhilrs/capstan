import CapstanAppCore
import CapstanKit
import Foundation
import UserNotifications

@MainActor final class NotificationCoordinator: NSObject, UNUserNotificationCenterDelegate {
    weak var model: AppModel?
    private let port: NotificationPort
    private let sounds: SoundStore
    /// The kinds that were on when the person last answered or launched; nil until `launched()`.
    private var knownKinds: Set<WaitingKind>?

    init(model: AppModel) {
        self.model = model
        self.port = NotificationCenterAdapter()
        self.sounds = UserDefaultsSoundStore()
        super.init()
    }

    /// Posts one notification per kind that is on: a title, the project name and a sound, no agent text. A stuck PM is
    /// announced when any kind is on; its recovery withdraws the notification of that episode.
    func handle(_ candidates: [NotifyCandidate]) {
        guard let model else { return }
        let settings = model.settings
        let preferences = SoundPreferences.load(from: sounds)
        var plans: [NotificationPlan] = []
        var withdrawn: [String] = []
        for candidate in candidates {
            let name = model.project(candidate.hostId, candidate.projectId)?.displayName ?? ""
            switch candidate.topic {
            case .waiting:
                guard settings.notifyKinds.contains(candidate.kind) else { continue }
                plans.append(
                    NotificationPlan(
                        candidate: candidate, settings: settings, projectName: name,
                        preferences: preferences, fileCheck: SoundLibrary.userFileUsable))
            case .pmStuck:
                guard !settings.notifyKinds.isEmpty else { continue }
                plans.append(
                    NotificationPlan(
                        pmStuck: candidate, settings: settings, projectName: name,
                        preferences: preferences, fileCheck: SoundLibrary.userFileUsable))
            case .pmRecovered(let episodeId):
                withdrawn.append(
                    NotificationPlan.pmStuckIdentifier(
                        hostId: candidate.hostId, projectId: candidate.projectId, episodeId: episodeId))
            }
        }
        guard !plans.isEmpty || !withdrawn.isEmpty else { return }
        let port = port
        Task {
            if !withdrawn.isEmpty { await port.remove(identifiers: withdrawn) }
            for plan in plans { await port.add(plan) }
        }
    }

    func settingsChanged(_ settings: AppSettings) {
        guard let previous = knownKinds else { return }
        knownKinds = settings.notifyKinds
        requestIfNeeded(kinds: settings.notifyKinds, previous: previous)
    }

    func launched() {
        guard let model else { return }
        let kinds = model.settings.notifyKinds
        knownKinds = kinds
        requestIfNeeded(kinds: kinds, previous: nil)
    }

    private func requestIfNeeded(kinds: Set<WaitingKind>, previous: Set<WaitingKind>?) {
        let port = port
        Task {
            let status = await port.authorizationStatus()
            if AuthorizationPolicy.shouldRequest(status: status, kindsOn: kinds, previousKinds: previous) {
                _ = await port.requestAuthorization()
            }
        }
    }

    private func responseReceived(_ row: InboxRowID?) {
        model?.showMain(row: row)
    }

    /// A tap on a stuck-PM notification opens that project's overview.
    private func stuckTapped(hostId: UUID, projectId: String) {
        model?.showMain(row: nil)
        model?.selection = .project(hostId: hostId, projectId: projectId)
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        if let (hostId, projectId) = NotificationPlan.pmStuckProject(fromUserInfo: info) {
            await stuckTapped(hostId: hostId, projectId: projectId)
            return
        }
        await responseReceived(NotificationPlan.row(fromUserInfo: info))
    }
}
