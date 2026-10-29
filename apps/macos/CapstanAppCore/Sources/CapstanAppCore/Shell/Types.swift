import CapstanKit
import Foundation

/// WaitingKind is a plain enum with a text payload; settings keep sets of it. Equality comes from the kit.
extension WaitingKind: @retroactive Hashable {
    public func hash(into hasher: inout Hasher) {
        hasher.combine(wireName)
    }
}

/// One decision that is being sent: the host, the project and the request it answers.
public struct BusyKey: Hashable, Sendable {
    public let hostId: UUID
    public let projectId: String
    public let requestId: String

    public init(hostId: UUID, projectId: String, requestId: String) {
        self.hostId = hostId
        self.projectId = projectId
        self.requestId = requestId
    }
}

/// What the Hosts area shows: a host, and a project of it when there is one.
public struct HostsFocus: Hashable, Sendable {
    public let hostId: UUID
    public let projectId: String?

    public init(hostId: UUID, projectId: String? = nil) {
        self.hostId = hostId
        self.projectId = projectId
    }
}

/// The look of every window: the system's, or light or dark.
public enum AppearanceChoice: String, Codable, CaseIterable, Sendable {
    case system, light, dark

    public var title: String {
        switch self {
        case .system: return "System"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }
}

public struct AppSettings: Codable, Equatable, Sendable {
    public var appearance: AppearanceChoice
    public var clientLabel: String
    public var notifyKinds: Set<WaitingKind>
    public var allowDuringFocus: Bool
    public var heartbeatPulse: Bool

    /// Settings saved before a key existed decode with that key's default.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let base = AppSettings()
        appearance = try c.decodeIfPresent(AppearanceChoice.self, forKey: .appearance) ?? base.appearance
        clientLabel = try c.decodeIfPresent(String.self, forKey: .clientLabel) ?? base.clientLabel
        notifyKinds = try c.decodeIfPresent(Set<WaitingKind>.self, forKey: .notifyKinds) ?? base.notifyKinds
        allowDuringFocus = try c.decodeIfPresent(Bool.self, forKey: .allowDuringFocus) ?? base.allowDuringFocus
        heartbeatPulse = try c.decodeIfPresent(Bool.self, forKey: .heartbeatPulse) ?? base.heartbeatPulse
    }

    public init(
        appearance: AppearanceChoice = .system,
        clientLabel: String = "Mac",
        notifyKinds: Set<WaitingKind> = [.op, .prompt, .design, .message, .integration],
        allowDuringFocus: Bool = false,
        heartbeatPulse: Bool = true
    ) {
        self.appearance = appearance
        self.clientLabel = clientLabel
        self.notifyKinds = notifyKinds
        self.allowDuringFocus = allowDuringFocus
        self.heartbeatPulse = heartbeatPulse
    }
}

public enum SidebarSelection: Codable, Hashable, Sendable {
    case inbox
    case project(hostId: UUID, projectId: String)
    case agents(hostId: UUID, projectId: String)
    case plans(hostId: UUID, projectId: String)
    case hosts
    /// The per-project conversation with the PM; it needs a host that supports PM chat.
    case messages

    /// The selection itself while its host and project exist; the inbox when they are gone.
    public func resolved(against hosts: [HostModel]) -> SidebarSelection {
        switch self {
        case .inbox, .hosts, .messages:
            return self
        case .project(let hostId, let projectId),
            .agents(let hostId, let projectId),
            .plans(let hostId, let projectId):
            let present = hosts.contains { $0.hostId == hostId && $0.projects[projectId] != nil }
            return present ? self : .inbox
        }
    }
}

public enum AppSheet: Identifiable, Equatable, Sendable {
    case addHost(editing: HostConfig?)

    public var id: String {
        switch self {
        case .addHost(let editing):
            return "addHost-" + (editing.map { $0.id.uuidString } ?? "new")
        }
    }
}
