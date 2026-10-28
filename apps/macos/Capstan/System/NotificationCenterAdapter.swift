import CapstanAppCore
import Foundation
import UserNotifications

/// Maps the system's notification center to `NotificationPort`, so the app core never imports `UserNotifications`.
struct NotificationCenterAdapter: NotificationPort {
    func authorizationStatus() async -> AuthStatus {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .notDetermined: return .notDetermined
        case .denied: return .denied
        case .authorized: return .authorized
        case .provisional: return .provisional
        @unknown default: return .denied
        }
    }

    func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])) ?? false
    }

    func add(_ plan: NotificationPlan) async {
        let content = UNMutableNotificationContent()
        content.title = plan.title
        content.subtitle = plan.subtitle
        content.userInfo = plan.userInfo
        switch plan.interruption {
        case .active: content.interruptionLevel = .active
        case .timeSensitive: content.interruptionLevel = .timeSensitive
        }
        if let file = plan.sound?.fileName {
            content.sound = UNNotificationSound(named: UNNotificationSoundName(rawValue: file))
        }
        let request = UNNotificationRequest(identifier: plan.identifier, content: content, trigger: nil)
        try? await UNUserNotificationCenter.current().add(request)
    }
}
