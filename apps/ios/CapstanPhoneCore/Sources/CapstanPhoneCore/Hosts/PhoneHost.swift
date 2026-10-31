import Foundation

public enum PhoneHostError: Error, Sendable, Equatable {
    case hostname
    case port
    case user
    /// Same user, port and hostname as a saved host.
    case duplicate
    /// The id's alias is already registered; draw a new id.
    case aliasInUse

    /// A short sentence for a form.
    public var message: String {
        switch self {
        case .hostname: return "Use a host name or an IP address"
        case .port: return "Use a port from 1 to 65535"
        case .user: return "Use the user name on the host"
        case .duplicate: return "This host is already added"
        case .aliasInUse: return "Try again"
        }
    }
}

/// Where a host is, for the phone's own SSH client. The `HostConfig` with the same id holds the rest; its destination
/// is this host's alias. One key per phone is used for every host.
public struct PhoneHost: Codable, Sendable, Equatable, Identifiable {
    public var id: UUID
    public var hostname: String
    public var port: Int
    public var user: String

    public init(id: UUID, hostname: String, port: Int = 22, user: String) {
        self.id = id
        self.hostname = hostname
        self.port = port
        self.user = user
    }

    /// The `HostConfig.destination` for a host: `capstan-` and the first 12 hex digits of its id. Never shown; the UI
    /// and error text use `hostname:port`.
    public static func alias(for id: UUID) -> String {
        let hex = id.uuidString.lowercased().replacingOccurrences(of: "-", with: "")
        return "capstan-" + hex.prefix(12)
    }

    public var alias: String {
        PhoneHost.alias(for: id)
    }

    /// `hostname:port`, the way the host is shown.
    public var display: String {
        hostname.contains(":") ? "[\(hostname)]:\(port)" : "\(hostname):\(port)"
    }

    public func validate() -> [PhoneHostError] {
        var errors: [PhoneHostError] = []
        if !PhoneHost.isHostname(hostname) { errors.append(.hostname) }
        if !(1...65535).contains(port) { errors.append(.port) }
        if !PhoneHost.isUser(user) { errors.append(.user) }
        return errors
    }

    /// What makes two hosts the same: user, port and hostname (lowercased, without a trailing dot).
    var identity: String {
        var name = hostname.lowercased()
        if name.hasSuffix(".") { name.removeLast() }
        return "\(user)@\(name):\(port)"
    }

    // A DNS name, an IPv4 address or an IPv6 address (with an optional zone): 1 to 253 bytes of letters, digits and
    // . - : %, not starting with - or . (so it is never read as an ssh option).
    private static func isHostname(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard let first = bytes.first, bytes.count <= 253, first != 0x2D, first != 0x2E else { return false }
        return bytes.allSatisfy { isAlphanumeric($0) || $0 == 0x2E || $0 == 0x2D || $0 == 0x3A || $0 == 0x25 }
    }

    // 1 to 64 bytes of letters, digits and . _ -, not starting with -.
    private static func isUser(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard let first = bytes.first, bytes.count <= 64, first != 0x2D else { return false }
        return bytes.allSatisfy { isAlphanumeric($0) || $0 == 0x2E || $0 == 0x5F || $0 == 0x2D }
    }

    private static func isAlphanumeric(_ byte: UInt8) -> Bool {
        (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
    }
}
