import Foundation

/// The projects the person pinned and the ones they opened last, kept on this device only. A project is named by
/// `hostId/projectId`, the id of `AgentGroup` and `PlanGroup`. Pinned projects come first on Agents and Plans.
public enum ProjectPins {
    public static let pinnedKey = "projects.pinned"
    public static let recentKey = "projects.recent"
    /// How many recent projects the jump sheet keeps.
    public static let recentCap = 5

    /// The ids in a stored value, in order.
    public static func list(_ raw: String) -> [String] {
        raw.split(separator: "\n").map(String.init)
    }

    public static func isPinned(_ id: String, in raw: String) -> Bool { list(raw).contains(id) }

    /// The stored value with `id` pinned if it was not, unpinned if it was.
    public static func toggled(_ id: String, in raw: String) -> String {
        var ids = list(raw)
        if let index = ids.firstIndex(of: id) { ids.remove(at: index) } else { ids.append(id) }
        return ids.joined(separator: "\n")
    }

    /// The stored value with `id` first, at most `recentCap` long.
    public static func touched(_ id: String, in raw: String) -> String {
        var ids = list(raw).filter { $0 != id }
        ids.insert(id, at: 0)
        return ids.prefix(recentCap).joined(separator: "\n")
    }
}

/// One project in the jump sheet: its name, host, a count ("7 agents", "34 plans") and its state in a word.
public struct JumpEntry: Identifiable, Sendable, Equatable {
    public let id: String
    public let name: String
    public let hostName: String
    public let count: String
    public let state: String
    public let tone: Tone

    public init(id: String, name: String, hostName: String, count: String, state: String, tone: Tone) {
        self.id = id
        self.name = name
        self.hostName = hostName
        self.count = count
        self.state = state
        self.tone = tone
    }
}
