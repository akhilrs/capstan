import Foundation
import LocalAuthentication

/// Asks the person to unlock. Tests and previews use a fake.
public protocol BiometricGate: Sendable {
    /// True when the person unlocked; false when they cancelled or the device cannot ask.
    func unlock(reason: String) async -> Bool
}

/// Face ID, with the device passcode as the system's fallback (so a Face ID lockout never locks the app for good).
public struct FaceIDGate: BiometricGate {
    public init() {}

    public func unlock(reason: String) async -> Bool {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else { return false }
        do {
            return try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        } catch {
            return false
        }
    }
}
