import Foundation

/// Where a notification, the Live Activity or a widget opens the app. Only ids travel in the URL, never host text:
///
///     capstan://item/<hostId>/<projectId>/<requestId>
///     capstan://agent/<hostId>/<projectId>/<agentId>
///     capstan://plan/<hostId>/<projectId>/<planId>
///     capstan://conversation/<hostId>/<projectId>
///
/// The host id is the host's UUID; the other ids are protocol ids (1 to 128 bytes of letters, digits and . _ : @ -,
/// not starting with - or .). Anything else does not parse.
public enum DeepLink: Sendable, Equatable {
    case item(hostId: UUID, projectId: String, requestId: String)
    case agent(hostId: UUID, projectId: String, agentId: String)
    case plan(hostId: UUID, projectId: String, planId: String)
    case conversation(hostId: UUID, projectId: String)

    public static let scheme = "capstan"
    public static let maxIdBytes = 128

    public var url: URL? {
        DeepLink.isWellFormed(self) ? URL(string: text) : nil
    }

    /// The URL text; check `url` for a link that may hold invalid ids.
    public var text: String {
        switch self {
        case .item(let host, let project, let request):
            return "\(DeepLink.scheme)://item/\(host.uuidString.lowercased())/\(project)/\(request)"
        case .agent(let host, let project, let agent):
            return "\(DeepLink.scheme)://agent/\(host.uuidString.lowercased())/\(project)/\(agent)"
        case .plan(let host, let project, let plan):
            return "\(DeepLink.scheme)://plan/\(host.uuidString.lowercased())/\(project)/\(plan)"
        case .conversation(let host, let project):
            return "\(DeepLink.scheme)://conversation/\(host.uuidString.lowercased())/\(project)"
        }
    }

    /// Parses the URL text strictly: the scheme, one of the four shapes, and valid ids. No query, no fragment, no
    /// percent escapes, no user or port.
    public init?(text: String) {
        let prefix = DeepLink.scheme + "://"
        guard text.utf8.count <= 512, text.hasPrefix(prefix) else { return nil }
        let parts = text.dropFirst(prefix.count).split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        guard let kind = parts.first, parts.count > 1, let host = UUID(uuidString: parts[1]) else { return nil }
        let ids = Array(parts.dropFirst(2))
        guard ids.allSatisfy(DeepLink.isId) else { return nil }
        switch (kind, ids.count) {
        case ("item", 2): self = .item(hostId: host, projectId: ids[0], requestId: ids[1])
        case ("agent", 2): self = .agent(hostId: host, projectId: ids[0], agentId: ids[1])
        case ("plan", 2): self = .plan(hostId: host, projectId: ids[0], planId: ids[1])
        case ("conversation", 1): self = .conversation(hostId: host, projectId: ids[0])
        default: return nil
        }
    }

    public init?(url: URL) {
        self.init(text: url.absoluteString)
    }

    public static func isId(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard let first = bytes.first, bytes.count <= maxIdBytes, first != 0x2D, first != 0x2E else { return false }
        return bytes.allSatisfy { byte in
            (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || byte == 0x2E || byte == 0x5F || byte == 0x3A || byte == 0x40 || byte == 0x2D
        }
    }

    private static func isWellFormed(_ link: DeepLink) -> Bool {
        switch link {
        case .item(_, let project, let id), .agent(_, let project, let id), .plan(_, let project, let id):
            return isId(project) && isId(id)
        case .conversation(_, let project):
            return isId(project)
        }
    }
}
