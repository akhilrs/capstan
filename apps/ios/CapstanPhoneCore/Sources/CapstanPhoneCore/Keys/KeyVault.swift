import Foundation

public enum KeyVaultError: Error, Sendable, Equatable {
    /// No Secure Enclave (the Simulator, or a device without one). Never answered with a silent ed25519 key.
    case enclaveUnavailable
    /// `create` was called while the phone already has a key; use `replace`.
    case keyExists
    /// A stored key could not be read back.
    case unreadable
    /// A Keychain call failed with this status.
    case keychain(Int32)
}

/// Where the phone's one SSH key lives. Calls block briefly on the Keychain; call them off the main thread when it
/// matters.
public protocol KeyVault: Sendable {
    /// The phone's key, or nil when there is none (first launch, or a restore that left the enclave key behind).
    func current() throws -> PhoneKey?
    /// Makes the phone's key. Throws `keyExists` when there is one already.
    func create(_ kind: PhoneKeyKind) throws -> PhoneKey
    /// Removes the phone's key; no key is not an error.
    func delete() throws
    /// Makes a new key of `kind` and then removes the old one. When making the new key fails, the old key stays.
    func replace(_ kind: PhoneKeyKind) throws -> PhoneKey
}
