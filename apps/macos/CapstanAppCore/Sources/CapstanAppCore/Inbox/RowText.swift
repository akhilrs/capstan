import CapstanKit
import Foundation

/// The state of an integration item, read from the first word of its summary (`<state> <branch>`).
public enum IntegrationState: Sendable, Equatable {
    case merged, conflicted, other
}

/// The second line of a row: a command is mono, the rest is plain text.
public enum RowSecondary: Sendable, Equatable {
    case mono(String)
    case plain(String)
    case unverified
}

/// The title and second line of an inbox row, as the design writes them: `project · actor`.
public enum RowText {
    public static let projectCap = 64
    public static let previewCap = 200

    public static func integrationState(_ row: InboxRow) -> IntegrationState {
        switch words(row).state {
        case "merged": return .merged
        case "conflicted": return .conflicted
        default: return .other
        }
    }

    /// The branch of an integration item; empty when the summary has none.
    public static func branch(_ row: InboxRow) -> String {
        words(row).branch
    }

    /// `project · actor` or `project · what it asks`. The feed does not carry an actor for a prompt or a message, so
    /// the title names what the item is.
    public static func title(_ row: InboxRow) -> String {
        let project = TextBudget.displayPrefix(row.projectName, maxBytes: projectCap).text
        return project + " · " + detail(row)
    }

    private static func detail(_ row: InboxRow) -> String {
        switch row.kind {
        case .op: return "operator"
        case .prompt: return "an agent asks for permission"
        case .design: return designName(key: row.id.key) ?? "design approval"
        case .message: return "needs an answer"
        case .integration:
            let branch = TextBudget.displayPrefix(words(row).branch, maxBytes: projectCap).text
            return branch.isEmpty ? "integration" : branch
        case .other(let name): return TextBudget.displayPrefix(name, maxBytes: projectCap).text
        }
    }

    /// `design:8/onboarding` as `plan-8 / onboarding`.
    public static func designName(key: String) -> String? {
        guard key.hasPrefix("design:") else { return nil }
        let rest = key.dropFirst("design:".count)
        let parts = rest.split(separator: "/", maxSplits: 1, omittingEmptySubsequences: false)
        guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
        return "plan-\(parts[0]) / \(parts[1])"
    }

    /// The kind as the inspector's caption names it.
    public static func kindName(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "Command proposal"
        case .prompt: return "Permission prompt"
        case .design: return "Design approval"
        case .message: return "Message"
        case .integration: return "Ready to merge"
        case .other(let name): return name
        }
    }

    /// The headline of the inspector.
    public static func inspectorTitle(kind: WaitingKind, key: String, summary: String) -> String {
        switch kind {
        case .op: return "The operator wants to run a command"
        case .prompt: return "An agent asks for permission"
        case .design: return (designName(key: key) ?? "The design") + " is ready"
        case .message: return "A message needs an answer"
        case .integration:
            let text = summary.trimmingCharacters(in: .whitespaces)
            let branch = text.firstIndex(of: " ").map { String(text[text.index(after: $0)...]).trimmingCharacters(in: .whitespaces) } ?? ""
            let state = text.split(separator: " ").first.map(String.init) ?? ""
            let name = branch.isEmpty ? "The integration" : branch
            return name + (state == "merged" ? " is ready to merge" : " has a merge conflict")
        case .other(let name): return name
        }
    }

    public static func secondary(_ row: InboxRow) -> RowSecondary {
        guard row.summary.verified else { return .unverified }
        switch row.kind {
        case .integration:
            switch integrationState(row) {
            case .merged: return .plain("Merged, ready for you to review")
            case .conflicted: return .plain("conflicted")
            case .other: return .plain(row.summary.displayPlain)
            }
        case .op:
            return .mono(TextBudget.displayPrefix(row.summary.displayPlain, maxBytes: previewCap).text)
        default:
            return .plain(TextBudget.displayPrefix(row.summary.displayPlain, maxBytes: previewCap).text)
        }
    }

    private static func words(_ row: InboxRow) -> (state: String, branch: String) {
        let text = row.summary.displayPlain.trimmingCharacters(in: .whitespaces)
        guard let space = text.firstIndex(of: " ") else { return (text, "") }
        let rest = text[text.index(after: space)...].trimmingCharacters(in: .whitespaces)
        return (String(text[..<space]), rest)
    }
}

/// A group of the inbox as shown. Ready-to-merge and conflicted integrations are two groups.
public struct DisplayGroup: Identifiable, Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case kind(WaitingKind)
        case mergeConflicts
    }

    public let kind: Kind
    public let rows: [InboxRow]

    public var id: String {
        switch kind {
        case .kind(let k): return k.wireName
        case .mergeConflicts: return "merge-conflicts"
        }
    }

    public var count: Int { rows.count }

    public var title: String {
        switch kind {
        case .mergeConflicts: return "Merge conflicts"
        case .kind(let k):
            switch k {
            case .op: return "Command proposals"
            case .prompt: return "Permission prompts"
            case .design: return "Design approvals"
            case .message: return "Messages"
            case .integration: return "Ready to merge"
            case .other(let name): return name
            }
        }
    }

    /// `Command proposals · 2`.
    public var header: String { "\(title) · \(count)" }

    /// A quiet group is listed last and its rows are not actions waiting for the person.
    public var quiet: Bool { kind == .mergeConflicts }

    public init(kind: Kind, rows: [InboxRow]) {
        self.kind = kind
        self.rows = rows
    }
}

public enum InboxLayout {
    /// The kit's groups with the integrations split: those in state `merged` stay in "Ready to merge", the others go
    /// to "Merge conflicts" at the end. Empty groups are left out.
    public static func groups(_ groups: [InboxGroup]) -> [DisplayGroup] {
        var shown: [DisplayGroup] = []
        var conflicted: [InboxRow] = []
        for group in groups {
            if case .integration = group.kind {
                let ready = group.rows.filter { RowText.integrationState($0) == .merged }
                conflicted += group.rows.filter { RowText.integrationState($0) != .merged }
                if !ready.isEmpty { shown.append(DisplayGroup(kind: .kind(.integration), rows: ready)) }
            } else if !group.rows.isEmpty {
                shown.append(DisplayGroup(kind: .kind(group.kind), rows: group.rows))
            }
        }
        if !conflicted.isEmpty { shown.append(DisplayGroup(kind: .mergeConflicts, rows: conflicted)) }
        return shown
    }
}

/// The filter of the toolbar: All, Proposals, Prompts, Designs, Messages and Merges.
public enum KindTab: CaseIterable, Sendable, Equatable {
    case all, proposals, prompts, designs, messages, merges

    public static let visible: [KindTab] = [.all, .proposals, .prompts, .designs, .messages, .merges]

    public var title: String {
        switch self {
        case .all: return "All"
        case .proposals: return "Proposals"
        case .prompts: return "Prompts"
        case .designs: return "Designs"
        case .messages: return "Messages"
        case .merges: return "Merges"
        }
    }

    public var kind: WaitingKind? {
        switch self {
        case .all: return nil
        case .proposals: return .op
        case .prompts: return .prompt
        case .designs: return .design
        case .messages: return .message
        case .merges: return .integration
        }
    }

    public init(kind: WaitingKind?) {
        switch kind {
        case nil: self = .all
        case .op?: self = .proposals
        case .prompt?: self = .prompts
        case .design?: self = .designs
        case .message?: self = .messages
        case .integration?, .other?: self = .merges
        }
    }
}

public struct ProjectKey: Hashable, Sendable {
    public let hostId: UUID
    public let projectId: String

    public init(hostId: UUID, projectId: String) {
        self.hostId = hostId
        self.projectId = projectId
    }
}

/// The numbers of the sidebar: aggregates across hosts and the waiting count of each project. They count the rows the
/// inbox lists (forwarded ones too), so a count never differs from the rows shown.
public struct SidebarCounts: Sendable, Equatable {
    public let waiting: Int
    public let agents: Int
    public let plans: Int
    public let messages: Int
    public let perProject: [ProjectKey: Int]

    public static func make(_ hosts: [HostModel]) -> SidebarCounts {
        let rows = inbox(hosts).flatMap(\.rows)
        var per: [ProjectKey: Int] = [:]
        for row in rows { per[ProjectKey(hostId: row.id.hostId, projectId: row.id.projectId), default: 0] += 1 }
        let projects = hosts.flatMap { $0.projects.values }
        return SidebarCounts(
            waiting: rows.count,
            agents: projects.reduce(0) { $0 + $1.agents.count },
            plans: projects.reduce(0) { $0 + $1.plans.count },
            messages: rows.filter { $0.kind == .message }.count,
            perProject: per)
    }
}

public enum SidebarText {
    /// The quiet word of a host row: live, reconnecting, no key.
    public static func hostStatus(state: ConnectionState?, projects: [ProjectModel]) -> (word: String, tone: Tone) {
        guard let state else { return ("offline", .grey) }
        switch state {
        case .connecting: return ("connecting", .yellow)
        case .live:
            if !projects.isEmpty, projects.allSatisfy({ $0.availability == .noKey }) { return ("no key", .grey) }
            return ("live", .green)
        case .noHeartbeat: return ("no heartbeat", .yellow)
        case .reconnecting: return ("reconnecting", .yellow)
        case .stopped(.user): return ("stopped", .grey)
        case .stopped: return ("stopped", .red)
        }
    }

    /// The quiet word of a project that is not ready; nil for a ready project, which shows its count.
    public static func projectStatus(_ availability: ProjectAvailability) -> String? {
        availability == .ready ? nil : AvailabilityText(availability).word.lowercased()
    }
}

/// The footer status capsule: one line for every host. The host name is set bold, the rest in secondary text.
public struct FooterStatus: Sendable, Equatable {
    /// The host the line is about; nil when it stands for several hosts that are all live.
    public let name: String?
    public let detail: String
    public let tone: Tone
    /// The state the capsule stands for, for the heartbeat pulse; nil without hosts.
    public let state: ConnectionState?
    public let loading: Bool

    public var text: String { name.map { $0 + " " + detail } ?? detail }

    /// The dot is filled when the host is live or stopped and a ring while it is connecting, late or reconnecting.
    public var ring: Bool {
        switch state {
        case .noHeartbeat?, .reconnecting?: return true
        default: return false
        }
    }

    public struct Host: Sendable {
        public let name: String
        public let state: ConnectionState?
        public let loading: Bool

        public init(name: String, state: ConnectionState?, loading: Bool) {
            self.name = name
            self.state = state
            self.loading = loading
        }
    }

    /// One host: `name live · heartbeat 3 s ago`. Several: the worst state, or `N hosts live` when all are live.
    public static func make(_ hosts: [Host], now: Date) -> FooterStatus {
        guard !hosts.isEmpty else {
            return FooterStatus(name: nil, detail: "No hosts", tone: .grey, state: nil, loading: false)
        }
        let live = hosts.filter { if case .live? = $0.state { return true } else { return false } }
        if hosts.count > 1, live.count == hosts.count {
            let loading = hosts.contains { $0.loading }
            return FooterStatus(
                name: nil, detail: loading ? "\(hosts.count) hosts live · loading" : "\(hosts.count) hosts live",
                tone: loading ? .blue : .green, state: live[0].state, loading: loading)
        }
        let worst = hosts.max { severity($0.state) < severity($1.state) } ?? hosts[0]
        let tone = CapsuleChoice.make(state: worst.state, anyLoading: worst.loading, heartbeatPulse: false, reduceMotion: true).tone
        return FooterStatus(
            name: HostText.name(worst.name), detail: phrase(worst, now: now), tone: tone, state: worst.state,
            loading: worst.loading)
    }

    private static func severity(_ state: ConnectionState?) -> Int {
        switch state {
        case .live?: return 0
        case .connecting?: return 1
        case .noHeartbeat?: return 2
        case .reconnecting?: return 3
        case .stopped(.user)?: return 4
        case .stopped?: return 5
        case nil: return 4
        }
    }

    private static func phrase(_ host: Host, now: Date) -> String {
        switch host.state {
        case nil: return "not connected"
        case .connecting?: return "connecting"
        case .live(_, let last)?:
            if host.loading { return "live · loading" }
            return "live · updated \(max(0, Int(now.timeIntervalSince(last)))) s ago"
        case .noHeartbeat(let since)?: return "no heartbeat for \(max(0, Int(now.timeIntervalSince(since)))) s"
        case .reconnecting(let attempt, let retryAt, _)?:
            return "reconnecting · try \(attempt) in \(max(0, Int(retryAt.timeIntervalSince(now).rounded(.up)))) s"
        case .stopped?: return "stopped"
        }
    }
}
