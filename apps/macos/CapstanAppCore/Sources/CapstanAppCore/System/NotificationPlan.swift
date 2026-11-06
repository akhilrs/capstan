import CapstanKit
import Foundation

public enum InterruptionLevel: Equatable, Sendable {
    case active
    /// Asks to break through a Focus. Without the entitlement macOS delivers the notification at the active level.
    case timeSensitive
}

/// What a notification for a new waiting item holds. It has a title from the kind, the project name as subtitle and
/// no body: no agent text ever leaves the app in a notification.
public struct NotificationPlan: Equatable, Sendable {
    public static let subtitleCap = 64

    public let identifier: String
    public let title: String
    public let subtitle: String
    public let interruption: InterruptionLevel
    /// nil for None.
    public let sound: SoundChoice?
    public let userInfo: [String: String]

    public init(
        candidate: NotifyCandidate, settings: AppSettings, projectName: String,
        preferences: SoundPreferences = SoundPreferences(), fileCheck: (String) -> Bool = { _ in true }
    ) {
        identifier = "waiting.\(candidate.hostId.uuidString).\(candidate.projectId).\(candidate.key)"
        title = NotificationPlan.title(candidate.kind)
        let plain = UntrustedText(raw: projectName, verified: true, truncated: false).displayPlain
        subtitle = String(plain.prefix(NotificationPlan.subtitleCap))
        interruption = settings.allowDuringFocus ? .timeSensitive : .active
        sound = preferences.resolve(candidate.kind, fileCheck: fileCheck)
        userInfo = [
            "hostId": candidate.hostId.uuidString, "projectId": candidate.projectId, "key": candidate.key,
        ]
    }

    /// The notification of a stuck PM: a fixed title, the project name and no body. The identifier names the episode, so
    /// one episode is one notification and its end can withdraw it. `userInfo` holds no `key`: it is not a row.
    public init(
        pmStuck candidate: NotifyCandidate, settings: AppSettings, projectName: String,
        preferences: SoundPreferences = SoundPreferences(), fileCheck: (String) -> Bool = { _ in true }
    ) {
        identifier = NotificationPlan.pmStuckIdentifier(
            hostId: candidate.hostId, projectId: candidate.projectId, episodeId: candidate.key)
        title = NotificationPlan.pmStuckTitle
        let plain = UntrustedText(raw: projectName, verified: true, truncated: false).displayPlain
        subtitle = String(plain.prefix(NotificationPlan.subtitleCap))
        interruption = .active
        sound = preferences.resolve(.message, fileCheck: fileCheck)
        userInfo = [
            "hostId": candidate.hostId.uuidString, "projectId": candidate.projectId, "pmStuck": candidate.key,
        ]
    }

    public static let pmStuckTitle = "PM looks stuck"

    public static func pmStuckIdentifier(hostId: UUID, projectId: String, episodeId: String) -> String {
        "pmstuck.\(hostId.uuidString).\(projectId).\(episodeId)"
    }

    /// The project a stuck-PM notification names; nil for any other notification.
    public static func pmStuckProject(fromUserInfo info: [AnyHashable: Any]) -> (UUID, String)? {
        guard info["pmStuck"] as? String != nil, let host = info["hostId"] as? String,
            let hostId = UUID(uuidString: host), let projectId = info["projectId"] as? String
        else { return nil }
        return (hostId, projectId)
    }

    public static func title(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "Command proposal waiting"
        case .prompt: return "Permission prompt waiting"
        case .design: return "Design approval waiting"
        case .message: return "Message waiting"
        case .integration: return "Ready to merge"
        case .other: return "Item waiting"
        }
    }

    /// The row a notification names; nil when one of the three values is missing or the host id is not an id.
    public static func row(fromUserInfo info: [AnyHashable: Any]) -> InboxRowID? {
        guard let host = info["hostId"] as? String, let hostId = UUID(uuidString: host),
            let projectId = info["projectId"] as? String, let key = info["key"] as? String
        else { return nil }
        return InboxRowID(hostId: hostId, projectId: projectId, key: key)
    }
}
