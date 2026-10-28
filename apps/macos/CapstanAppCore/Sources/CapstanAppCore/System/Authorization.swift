import CapstanKit
import Foundation

public enum AuthStatus: Equatable, Sendable {
    case notDetermined, denied, authorized, provisional, ephemeral
}

/// The part of the system's notification center the app uses. `UserNotifications` is imported only by the adapter in
/// the app, so this module does not depend on it.
public protocol NotificationPort: Sendable {
    func authorizationStatus() async -> AuthStatus
    func requestAuthorization() async -> Bool
    func add(_ plan: NotificationPlan) async
}

public enum AuthorizationPolicy {
    /// Whether to show the system's permission prompt. At launch (`previousKinds` nil) when any kind is on; later
    /// when a kind has been turned on since `previousKinds`. Never once the person has answered.
    public static func shouldRequest(
        status: AuthStatus, kindsOn: Set<WaitingKind>, previousKinds: Set<WaitingKind>?
    ) -> Bool {
        guard status == .notDetermined else { return false }
        guard let previousKinds else { return !kindsOn.isEmpty }
        return !kindsOn.subtracting(previousKinds).isEmpty
    }
}
