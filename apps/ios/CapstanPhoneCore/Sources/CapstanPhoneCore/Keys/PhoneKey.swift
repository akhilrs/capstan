import CryptoKit
import Foundation

/// The kinds of key the iPhone can sign with. One key per phone, used for every host.
public enum PhoneKeyKind: String, Sendable, Codable, CaseIterable {
    /// P-256 created in the Secure Enclave: never leaves the device, gone after a restore. The default.
    case secureEnclaveP256
    /// ed25519 kept in the Keychain, for hosts that accept only ed25519. Can be backed up and restored elsewhere.
    case keychainEd25519
}

/// A public key as SSH sees it.
// CryptoKit's key types are value types with no shared mutable state.
public enum PhonePublicKey: @unchecked Sendable, Equatable {
    case p256(P256.Signing.PublicKey)
    case ed25519(Curve25519.Signing.PublicKey)

    public static func == (lhs: PhonePublicKey, rhs: PhonePublicKey) -> Bool {
        switch (lhs, rhs) {
        case (.p256(let a), .p256(let b)): return a.x963Representation == b.x963Representation
        case (.ed25519(let a), .ed25519(let b)): return a.rawRepresentation == b.rawRepresentation
        default: return false
        }
    }
}

/// The phone's private key. The enclave key's bytes never leave the enclave; this wraps a handle to it.
// CryptoKit's key types are value types with no shared mutable state.
public struct PhoneKey: @unchecked Sendable {
    public enum Backing {
        case secureEnclaveP256(SecureEnclave.P256.Signing.PrivateKey)
        case keychainEd25519(Curve25519.Signing.PrivateKey)
    }

    public let backing: Backing

    public init(_ backing: Backing) {
        self.backing = backing
    }

    public var kind: PhoneKeyKind {
        switch backing {
        case .secureEnclaveP256: return .secureEnclaveP256
        case .keychainEd25519: return .keychainEd25519
        }
    }

    public var publicKey: PhonePublicKey {
        switch backing {
        case .secureEnclaveP256(let key): return .p256(key.publicKey)
        case .keychainEd25519(let key): return .ed25519(key.publicKey)
        }
    }
}
