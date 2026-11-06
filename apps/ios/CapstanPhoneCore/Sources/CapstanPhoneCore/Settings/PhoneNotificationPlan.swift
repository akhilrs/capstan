import CapstanAppCore
import CapstanKit
import Foundation

/// A notification for a new waiting item on the phone. Its title and body are `CarTexts` (the kind and the project name, so
/// Siri can read them in the car); the project name and the sound come from the Mac's `NotificationPlan`. No agent text is ever part of it.
public struct PhoneNotificationPlan: Equatable, Sendable {
    public let identifier: String
    /// One category per kind, so the system can group and the person can manage them.
    public let category: String
    public let title: String
    /// "In <project>.", the project name plain and capped.
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
        title = CarTexts.title(candidate.kind)
        body = CarTexts.body(project: base.subtitle)
        threadIdentifier = "project.\(candidate.hostId.uuidString).\(candidate.projectId)"
        timeSensitive = settings.timeSensitiveKinds.contains(candidate.kind)
        sound = base.sound
        deepLink = DeepLink.item(hostId: candidate.hostId, projectId: candidate.projectId, requestId: requestId)
            .url?.absoluteString
    }

    /// The category of the stuck-PM notification.
    public static let pmStuckCategory = "pm.stuck"

    /// A stuck PM: the Mac's title and project name, no agent text. nil when every kind is switched off.
    public init?(
        pmStuck hostId: UUID, projectId: String, episodeId: String, projectName: String, settings: PhoneSettings,
        fileCheck: (String) -> Bool = { _ in true }
    ) {
        guard !settings.notifyKinds.isEmpty else { return nil }
        let candidate = NotifyCandidate(
            hostId: hostId, projectId: projectId, kind: .other("pm_stuck"), key: episodeId,
            topic: .pmStuck(episodeId: episodeId))
        let base = NotificationPlan(
            pmStuck: candidate, settings: AppSettings(), projectName: projectName, preferences: settings.sounds,
            fileCheck: fileCheck)
        identifier = base.identifier
        category = PhoneNotificationPlan.pmStuckCategory
        title = base.title
        body = base.subtitle
        threadIdentifier = "project.\(hostId.uuidString).\(projectId)"
        timeSensitive = false
        sound = base.sound
        deepLink = DeepLink.conversation(hostId: hostId, projectId: projectId).url?.absoluteString
    }

    public static func category(_ kind: WaitingKind) -> String {
        switch kind {
        case .other: return "waiting.other"
        default: return "waiting." + kind.wireName
        }
    }

    /// The categories to register at launch.
    public static let categories = ["op", "prompt", "design", "message", "integration", "other"].map { "waiting." + $0 }
        + [pmStuckCategory]
}
