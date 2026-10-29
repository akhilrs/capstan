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

    /// Posts one notification per kind that is on: a title, the project name and a sound, no agent text.
    func handle(_ candidates: [NotifyCandidate]) {
        guard let model else { return }
        let settings = model.settings
        let preferences = SoundPreferences.load(from: sounds)
        let plans = candidates.filter { settings.notifyKinds.contains($0.kind) }.map { candidate in
            NotificationPlan(
                candidate: candidate, settings: settings,
                projectName: model.project(candidate.hostId, candidate.projectId)?.displayName ?? "",
                preferences: preferences, fileCheck: SoundLibrary.userFileUsable)
        }
        guard !plans.isEmpty else { return }
        let port = port
        Task {
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

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let row = NotificationPlan.row(fromUserInfo: response.notification.request.content.userInfo)
        await responseReceived(row)
    }
}
