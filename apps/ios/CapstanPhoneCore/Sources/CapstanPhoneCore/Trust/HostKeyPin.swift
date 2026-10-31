import CryptoKit
import Foundation

/// A host key the person trusted: one per host.
public struct HostKeyPin: Codable, Sendable, Equatable {
    /// The SSH algorithm name, for example `ssh-ed25519`.
    public let algorithm: String
    /// The public key blob as the host sent it.
    public let blob: Data
    /// `SHA256:` and the base64 digest of the blob without padding, as `ssh-keygen -lf` prints it.
    public let fingerprint: String

    public init(algorithm: String, blob: Data) {
        self.algorithm = algorithm
        self.blob = blob
        var digest = Data(SHA256.hash(data: blob)).base64EncodedString()
        while digest.hasSuffix("=") { digest.removeLast() }
        self.fingerprint = "SHA256:" + digest
    }
}

/// What to do with the key a host offered.
public enum TrustVerdict: Sendable, Equatable {
    /// The offered key is the pinned one.
    case trusted
    /// No pin for this host yet: ask the person, showing the fingerprint.
    case unknown(HostKeyPin)
    /// A pin exists and this is another key (or another algorithm): blocked, never "trust anyway".
    case changed(pinned: HostKeyPin, offered: HostKeyPin)
    /// The pin file exists but cannot be read: connect to nothing until `resetAll()`.
    case pinsUnreadable
}
