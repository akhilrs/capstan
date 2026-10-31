import CapstanAppCore
import CapstanKit
import Foundation

/// A notification for a new waiting item on the phone. It reuses the Mac's `NotificationPlan` for the title (from the
/// kind), the project name and the sound; the body is the project name. No agent text is ever part of it.
public struct PhoneNotificationPlan: Equatable, Sendable {
    public let identifier: String
    /// One category per kind, so the system can group and the person can manage them.
    public let category: String
    public let title: String
    /// The project name, plain and capped.
    public let body: String
    /// Notifications of one project are threaded together.
    public let threadIdentifier: String
    public let timeSensitive: Bool
    /// nil for None.
    public let sound: SoundChoice?
    /// Opens the item; nil when an id does not fit a deep link (the app then opens on Waiting).
    public let deepLink: String?

    /// nil when the person switched notifications off for the kind.
    public init?(
        candidate: NotifyCandidate, requestId: String, projectName: String, settings: PhoneSettings,
        fileCheck: (String) -> Bool = { _ in true }
    ) {
        guard settings.notifyKinds.contains(candidate.kind) else { return nil }
        let base = NotificationPlan(
            candidate: candidate, settings: AppSettings(), projectName: projectName, preferences: settings.sounds,
            fileCheck: fileCheck)
        identifier = base.identifier
        category = PhoneNotificationPlan.category(candidate.kind)
        title = base.title
        body = base.subtitle
        threadIdentifier = "project.\(candidate.hostId.uuidString).\(candidate.projectId)"
        timeSensitive = settings.timeSensitiveKinds.contains(candidate.kind)
        sound = base.sound
        deepLink = DeepLink.item(hostId: candidate.hostId, projectId: candidate.projectId, requestId: requestId)
            .url?.absoluteString
    }

    public static func category(_ kind: WaitingKind) -> String {
        switch kind {
        case .other: return "waiting.other"
        default: return "waiting." + kind.wireName
        }
    }

    /// The categories to register at launch.
    public static let categories = ["op", "prompt", "design", "message", "integration", "other"].map { "waiting." + $0 }
}
