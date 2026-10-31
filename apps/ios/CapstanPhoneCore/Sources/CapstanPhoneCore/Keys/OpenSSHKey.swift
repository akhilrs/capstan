import CryptoKit
import Foundation

/// The phone's public key in the forms `ssh-keygen` prints, for `authorized_keys` and for comparing fingerprints.
public enum OpenSSHKey {
    /// The SSH algorithm name of the key.
    public static func algorithm(_ key: PhonePublicKey) -> String {
        switch key {
        case .p256: return "ecdsa-sha2-nistp256"
        case .ed25519: return "ssh-ed25519"
        }
    }

    /// The public key blob (RFC 4253 section 6.6, RFC 5656 section 3.1, RFC 8709 section 4).
    public static func blob(_ key: PhonePublicKey) -> Data {
        var data = Data()
        appendString(Data(algorithm(key).utf8), to: &data)
        switch key {
        case .p256(let p256):
            appendString(Data("nistp256".utf8), to: &data)
            appendString(p256.x963Representation, to: &data)
        case .ed25519(let ed25519):
            appendString(ed25519.rawRepresentation, to: &data)
        }
        return data
    }

    /// One `authorized_keys` line, as `ssh-keygen -y` prints it: algorithm, base64 blob and the comment if any.
    public static func publicLine(_ key: PhonePublicKey, comment: String? = nil) -> String {
        var line = algorithm(key) + " " + blob(key).base64EncodedString()
        if let comment, !comment.isEmpty { line += " " + comment }
        return line
    }

    /// The fingerprint as `ssh-keygen -lf` prints it: `SHA256:` and the base64 digest without padding.
    public static func fingerprint(_ key: PhonePublicKey) -> String {
        let digest = Data(SHA256.hash(data: blob(key)))
        var text = digest.base64EncodedString()
        while text.hasSuffix("=") { text.removeLast() }
        return "SHA256:" + text
    }

    private static func appendString(_ bytes: Data, to data: inout Data) {
        var length = UInt32(bytes.count).bigEndian
        withUnsafeBytes(of: &length) { data.append(contentsOf: $0) }
        data.append(bytes)
    }
}
