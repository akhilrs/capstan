import CapstanAppCore
import CapstanPhoneCore
import Foundation
import UserNotifications

/// Posts the local notifications `RefreshPlanner` plans (kind and project name only, never agent text), keeps the
/// app badge at the number waiting, and opens the item when a notification is tapped. Local only: the app never
/// registers for remote notifications (D8).
@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    private let center = UNUserNotificationCenter.current()
    /// Called with the deep link of a tapped notification.
    var onOpen: ((URL) -> Void)?

    override init() {
        super.init()
        center.delegate = self
        center.setNotificationCategories(Set(PhoneNotificationPlan.categories.map {
            UNNotificationCategory(identifier: $0, actions: [], intentIdentifiers: [], options: [])
        }))
    }

    /// Asks once; iOS remembers the answer.
    func requestAuthorization() {
        center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    func post(_ plans: [PhoneNotificationPlan]) {
        for plan in plans {
            let content = UNMutableNotificationContent()
            content.title = plan.title
            content.body = plan.body
            content.categoryIdentifier = plan.category
            content.threadIdentifier = plan.threadIdentifier
            content.interruptionLevel = plan.timeSensitive ? .timeSensitive : .active
            content.sound = Notifier.sound(plan.sound)
            if let link = plan.deepLink { content.userInfo = ["link": link] }
            center.add(UNNotificationRequest(identifier: plan.identifier, content: content, trigger: nil))
        }
    }

    func setBadge(_ count: Int) {
        center.setBadgeCount(count)
    }

    /// A tone the person added plays by name from Library/Sounds; a system tone plays as the default alert sound,
    /// since iOS plays notification sounds only from the app's own folders.
    static func sound(_ choice: SoundChoice?) -> UNNotificationSound? {
        switch choice {
        case nil, .silent?: return nil
        case .user(let name)?: return UNNotificationSound(named: UNNotificationSoundName(name))
        case .system?: return .default
        }
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        guard let text = response.notification.request.content.userInfo["link"] as? String,
            let url = URL(string: text)
        else { return }
        await MainActor.run { self.onOpen?(url) }
    }
}
