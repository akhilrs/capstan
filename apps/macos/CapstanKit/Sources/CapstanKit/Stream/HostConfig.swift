import Foundation

public enum HostConfigError: Error, Sendable, Equatable {
    case destination
    case cstanPath
    case root(String)
    case pollMs
    case identityFile

    /// A short sentence for a settings form.
    public var message: String {
        switch self {
        case .destination: return "Use a host name, user@host or an ssh config alias"
        case .cstanPath: return "Use a plain command name or path"
        case .root(let root): return "Project root must be an absolute path: \(root)"
        case .pollMs: return "At least \(ProtocolLimits.minPollMs) ms"
        case .identityFile: return "Use the absolute path of a key file"
        }
    }
}

/// One remote host. Only the path of a key file is stored, never a key, a passphrase or a password.
public struct HostConfig: Codable, Sendable, Identifiable, Equatable {
    public var id: UUID
    public var displayName: String
    public var destination: String
    public var cstanPath: String
    public var extraRoots: [String]
    public var pollMs: Int
    public var identityFile: String?
    public var enabled: Bool

    public init(
        id: UUID = UUID(), displayName: String, destination: String, cstanPath: String = "cstan",
        extraRoots: [String] = [], pollMs: Int = 2000, identityFile: String? = nil, enabled: Bool = true
    ) {
        self.id = id
        self.displayName = displayName
        self.destination = destination
        self.cstanPath = cstanPath
        self.extraRoots = extraRoots
        self.pollMs = pollMs
        self.identityFile = identityFile
        self.enabled = enabled
    }

    public func validate() -> [HostConfigError] {
        var errors: [HostConfigError] = []
        if !HostConfig.isDestination(destination) { errors.append(.destination) }
        if !HostConfig.isCstanPath(cstanPath) { errors.append(.cstanPath) }
        for root in extraRoots where !HostConfig.isAbsolutePath(root) { errors.append(.root(root)) }
        if pollMs < ProtocolLimits.minPollMs { errors.append(.pollMs) }
        if let identityFile, !HostConfig.isAbsolutePath(identityFile) { errors.append(.identityFile) }
        return errors
    }

    // ^[A-Za-z0-9][A-Za-z0-9._@:%\[\]-]{0,254}$
    private static func isDestination(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard let first = bytes.first, bytes.count <= 255, isAlphanumeric(first) else { return false }
        return bytes.dropFirst().allSatisfy { byte in
            isAlphanumeric(byte) || "._@:%[]-".utf8.contains(byte)
        }
    }

    // ^(~/)?[A-Za-z0-9._/][A-Za-z0-9._/-]*$
    private static func isCstanPath(_ text: String) -> Bool {
        let body = text.hasPrefix("~/") ? String(text.dropFirst(2)) : text
        let bytes = Array(body.utf8)
        guard let first = bytes.first, isAlphanumeric(first) || "._/".utf8.contains(first) else { return false }
        return bytes.dropFirst().allSatisfy { isAlphanumeric($0) || "._/-".utf8.contains($0) }
    }

    private static func isAbsolutePath(_ text: String) -> Bool {
        guard text.hasPrefix("/") else { return false }
        return !text.unicodeScalars.contains { $0.value < 0x20 || $0.value == 0x7F }
    }

    private static func isAlphanumeric(_ byte: UInt8) -> Bool {
        (byte >= UInt8(ascii: "0") && byte <= UInt8(ascii: "9"))
            || (byte >= UInt8(ascii: "A") && byte <= UInt8(ascii: "Z"))
            || (byte >= UInt8(ascii: "a") && byte <= UInt8(ascii: "z"))
    }
}
