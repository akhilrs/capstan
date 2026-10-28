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

public struct AppSettings: Codable, Equatable, Sendable {
    public var clientLabel: String
    public var notifyKinds: Set<WaitingKind>
    public var allowDuringFocus: Bool
    public var heartbeatPulse: Bool

    public init(
        clientLabel: String = "Mac",
        notifyKinds: Set<WaitingKind> = [.op, .prompt, .design, .message, .integration],
        allowDuringFocus: Bool = false,
        heartbeatPulse: Bool = true
    ) {
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

    /// The selection itself while its host and project exist; the inbox when they are gone.
    public func resolved(against hosts: [HostModel]) -> SidebarSelection {
        switch self {
        case .inbox, .hosts:
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
