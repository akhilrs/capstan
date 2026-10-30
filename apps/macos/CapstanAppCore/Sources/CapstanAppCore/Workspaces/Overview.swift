import CapstanKit
import Foundation

// The project overview of the list column, as pure rules. The status word, the counts and the plan fact lines are
// built from counts and states only, never from text an agent wrote. Agent text appears only in the fields named for
// it (a task, a plan title), isolated and cleaned.

/// The word that stands for the whole project. The first that holds, in this order.
public enum OverviewStatus: Sendable, Equatable {
    case unknown, paused, needsAttention, moving, quiet

    public var word: String {
        switch self {
        case .unknown: return "Unknown"
        case .paused: return "Paused"
        case .needsAttention: return "Needs attention"
        case .moving: return "Moving"
        case .quiet: return "Quiet"
        }
    }
}

/// How many agents have one health word.
public struct OverviewCount: Sendable, Equatable {
    public let health: AgentHealth
    public let count: Int

    /// `3 working`, `1 needs you`.
    public var text: String { "\(count) \(health.word.lowercased())" }
}

public struct OverviewAgent: Sendable, Equatable, Identifiable {
    /// The agent's key in the snapshot, stable while rows reorder.
    public let id: String
    public let agentId: String
    public let role: String
    public let health: AgentHealth
    public let healthWord: String
    public let healthSymbol: String
    /// The task on one line, isolated; nil when there is none or the host could not vouch for it.
    public let task: String?
    public let taskUnverified: Bool
    /// `task of dev-4`: where the task text comes from.
    public let taskSource: String
    /// Only when the host sent one.
    public let branch: String?
    /// `6m`, for a Working agent whose time is readable and not in the future; nil otherwise.
    public let workingFor: String?
    /// The item waits for its re-read: the row is dimmed with its last-known value.
    public let isPending: Bool
}

public struct OverviewPlan: Sendable, Equatable, Identifiable {
    public let id: String
    public let planId: String
    public let sequence: Int64
    /// The title, isolated; the plan id when the host could not vouch for the title.
    public let title: String
    public let titleUnverified: Bool
    public let progress: PlanProgress
    /// `3 of 6 integrated`.
    public let progressLabel: String
    /// Facts from counts and state only.
    public let factLine: String
    public let isPending: Bool
}

public struct OverviewOutcome: Sendable, Equatable, Identifiable {
    public let id: String
    public let seq: Int64
    public let k: Int64
    public let type: String
    /// An SF Symbols name.
    public let symbol: String
    public let text: String
    /// When it happened on the host: `14:03`, `yesterday 18:40`, `now`. In the future (clock skew) it is the absolute
    /// time with no age.
    public let hostTime: String
    public let isSkewed: Bool
    /// When this Mac received it, apart from the host time.
    public let receivedTime: String
    public let isLive: Bool
}

public enum OverviewHistoryKind: Sendable, Equatable {
    case idle, loading, backfilled, sessionOnly, error, truncated
}

/// What the list column shows.
public enum ListColumnContent: Sendable, Equatable {
    /// A project is selected: the overview, with the waiting groups below its status strip when any wait.
    case overview
    /// The cross-project view with items waiting.
    case waitingGroups
    /// The cross-project view with nothing waiting.
    case emptyState
}

public struct OverviewModel: Sendable, Equatable {
    public static let agentsFolded = "agents"
    public static let plansFolded = "plans"
    public static let outcomesFolded = "outcomes"
    public static let maxExceptions = 4
    public static let maxWorking = 3
    public static let collapsedOutcomeCount = 5
    public static let scanLimitRows = 500
    /// The plan states that are in progress.
    static let planStates: Set<String> = ["submitted", "in_review", "approved", "signed_off"]

    public let status: OverviewStatus
    public var statusWord: String { status.word }
    /// Zero counts are left out. Empty while the counts are unknown.
    public let counts: [OverviewCount]
    public let countsKnown: Bool
    /// `3 working · 1 stalled`, or `–` while the counts are unknown.
    public let countsText: String
    public let freshnessLine: String
    /// The whole strip on one line: `Moving · 3 working · 2 idle · current as of 14:05`.
    public let statusLine: String
    /// The value VoiceOver reads for the strip.
    public let accessibilityValue: String
    /// A fact line under a Quiet strip, from counts only.
    public let quietFact: String?
    /// The sections show last-known values, dimmed and marked.
    public let isLastKnown: Bool

    public let agentsNow: [OverviewAgent]
    /// Exceptions beyond the four shown: `+3 more need attention`.
    public let moreNeedAttention: Int
    public let moreNeedAttentionText: String?
    public let totalAgents: Int
    public let viewAllText: String?
    public let agentsEmptyText: String

    public let plans: [OverviewPlan]
    public let plansEmptyText: String

    public let outcomes: [OverviewOutcome]
    public let historyState: OverviewHistoryKind
    public let historyText: String?
    public let historyGapText: String?
    public let outcomesEmptyText: String?
    public let canRetryHistory: Bool

    public let foldedSections: Set<String>
    public let showsOverview: Bool

    public func isFolded(_ section: String) -> Bool { foldedSections.contains(section) }

    /// The list column shows the overview for a selected project; the empty state only in the cross-project view.
    public static func listColumn(selectedProjectId: String?, anythingWaiting: Bool) -> ListColumnContent {
        if selectedProjectId != nil { return .overview }
        return anythingWaiting ? .waitingGroups : .emptyState
    }

    // MARK: Build

    public static func make(
        project: ProjectModel, freshness: ProjectFreshness, history: RecentHistoryState, folded: Set<String>,
        now: Date, calendar: Calendar = Calendar.current
    ) -> OverviewModel {
        let times = OverviewTimes(now: now, calendar: calendar)
        let rows: [AgentRow] = AgentRow.rows(project)
        let healths: [AgentHealth] = rows.map { (row: AgentRow) -> AgentHealth in row.health }
        let status = statusOf(project: project, freshness: freshness, healths: healths)
        let countsKnown = !freshness.isRefreshing
        let counts: [OverviewCount] = countsKnown ? countList(healths) : []
        let countsText = countsLine(counts, known: countsKnown)
        let freshnessLine = freshnessText(freshness, times: times)
        var parts: [String] = [status.word]
        if !countsText.isEmpty { parts.append(countsText) }
        if !freshnessLine.isEmpty { parts.append(freshnessLine) }

        let plans = planList(project, freshness: freshness)
        let agents = agentList(project, rows: rows, freshness: freshness, now: now)
        let outcomeList: [OverviewOutcome] = history.shown.map { (entry: HistoryEvent) -> OverviewOutcome in
            outcome(entry, times: times)
        }
        let historyPart = historyTexts(history, outcomes: outcomeList, times: times, now: now)

        return OverviewModel(
            status: status, counts: counts, countsKnown: countsKnown, countsText: countsText,
            freshnessLine: freshnessLine, statusLine: parts.joined(separator: " · "),
            accessibilityValue: spoken(status, counts: counts, countsKnown: countsKnown, freshness: freshnessLine),
            quietFact: status == .quiet ? quietFactLine(project, plans: plans, rows: rows) : nil,
            isLastKnown: freshness.showsLastKnown, agentsNow: agents.rows, moreNeedAttention: agents.more,
            moreNeedAttentionText: agents.more > 0 ? "+\(agents.more) more need attention" : nil,
            totalAgents: rows.count,
            viewAllText: rows.isEmpty ? nil : (rows.count == 1 ? "View all 1 agent" : "View all \(rows.count) agents"),
            agentsEmptyText: "No active agent", plans: plans, plansEmptyText: "No plan in progress",
            outcomes: outcomeList, historyState: historyPart.kind, historyText: historyPart.text,
            historyGapText: historyPart.gap, outcomesEmptyText: historyPart.empty,
            canRetryHistory: historyPart.canRetry, foldedSections: folded, showsOverview: true)
    }

    // MARK: Status

    private static func statusOf(project: ProjectModel, freshness: ProjectFreshness, healths: [AgentHealth]) -> OverviewStatus {
        if freshness.isRefreshing || freshness.refreshFailedAt != nil || !freshness.isReady { return .unknown }
        if project.run?.paused == true { return .paused }
        if healths.contains(where: { (health: AgentHealth) -> Bool in isException(health) && health != .needsYou }) {
            return .needsAttention
        }
        // Needs you alone is not Moving: those agents wait for the person and appear in the waiting groups.
        if healths.contains(.working) { return .moving }
        return .quiet
    }

    private static func isException(_ health: AgentHealth) -> Bool {
        switch health {
        case .lost, .blocked, .stalled, .needsYou: return true
        default: return false
        }
    }

    private static let countOrder: [AgentHealth] = [.working, .needsYou, .stalled, .blocked, .lost, .paused, .idle]

    private static func countList(_ healths: [AgentHealth]) -> [OverviewCount] {
        var list: [OverviewCount] = []
        for health in countOrder {
            let count = healths.filter { (other: AgentHealth) -> Bool in other == health }.count
            if count > 0 { list.append(OverviewCount(health: health, count: count)) }
        }
        return list
    }

    private static func countsLine(_ counts: [OverviewCount], known: Bool) -> String {
        if !known { return "–" }
        let texts: [String] = counts.map { (count: OverviewCount) -> String in count.text }
        return texts.joined(separator: " · ")
    }

    private static func spoken(
        _ status: OverviewStatus, counts: [OverviewCount], countsKnown: Bool, freshness: String
    ) -> String {
        var text = status.word
        if countsKnown, !counts.isEmpty {
            let texts: [String] = counts.map { (count: OverviewCount) -> String in count.text }
            text += ": " + texts.joined(separator: ", ")
        }
        text += "."
        if let first = freshness.first {
            let head = String(first).uppercased()
            let tail = String(freshness.dropFirst())
            text += " " + head + tail + "."
        }
        return text
    }

    private static func quietFactLine(_ project: ProjectModel, plans: [OverviewPlan], rows: [AgentRow]) -> String? {
        if plans.isEmpty {
            let integrated = project.plans.values.filter { (plan: StoredPlan) -> Bool in
                !planStates.contains(plan.state) && plan.packages.total > 0 && plan.packages.integrated >= plan.packages.total
            }.count
            switch integrated {
            case 0: return "No plan in progress"
            case 1: return "No plan in progress · 1 plan integrated"
            default: return "No plan in progress · \(integrated) plans integrated"
            }
        }
        let hasTask = rows.contains { (row: AgentRow) -> Bool in row.task != nil }
        return hasTask ? nil : "No active agent has a task"
    }

    // MARK: Freshness

    private static func freshnessText(_ freshness: ProjectFreshness, times: OverviewTimes) -> String {
        if !freshness.isReady || freshness.liveness == .silent {
            guard let known = freshness.lastKnownAt ?? freshness.currentAsOf else { return "last known" }
            return "last known, " + times.text(known)
        }
        if freshness.isRefreshing { return "Refreshing…" }
        if let failed = freshness.refreshFailedAt {
            var text = "Refresh failed at " + times.text(failed)
            if let known = freshness.lastKnownAt { text += " · showing last known from " + times.text(known) }
            return text
        }
        if let current = freshness.currentAsOf {
            var text = "current as of " + times.text(current)
            let pending = freshness.pendingKeys.count
            if pending > 0 { text += pending == 1 ? " · 1 item updating" : " · \(pending) items updating" }
            return text
        }
        if let known = freshness.lastKnownAt { return "last known from " + times.text(known) }
        return ""
    }

    // MARK: Agents

    private static func exceptionRank(_ health: AgentHealth) -> Int {
        switch health {
        case .lost: return 0
        case .blocked: return 1
        case .stalled: return 2
        default: return 3
        }
    }

    private static func agentList(
        _ project: ProjectModel, rows: [AgentRow], freshness: ProjectFreshness, now: Date
    ) -> (rows: [OverviewAgent], more: Int) {
        let exceptions: [AgentRow] = rows.filter { (row: AgentRow) -> Bool in isException(row.health) }
            .sorted { (left: AgentRow, right: AgentRow) -> Bool in
                let a = exceptionRank(left.health)
                let b = exceptionRank(right.health)
                return a == b ? left.agentId < right.agentId : a < b
            }
        let working: [AgentRow] = rows.filter { (row: AgentRow) -> Bool in row.health == .working }
            .sorted { (left: AgentRow, right: AgentRow) -> Bool in
                let a = OverviewTimes.date(left.source.activitySince)
                let b = OverviewTimes.date(right.source.activitySince)
                switch (a, b) {
                case (let x?, let y?): return x == y ? left.agentId < right.agentId : x < y
                case (_?, nil): return true
                case (nil, _?): return false
                case (nil, nil): return left.agentId < right.agentId
                }
            }
        let shownExceptions = Array(exceptions.prefix(maxExceptions))
        let shown: [AgentRow] = shownExceptions + Array(working.prefix(maxWorking))
        let list: [OverviewAgent] = shown.map { (row: AgentRow) -> OverviewAgent in
            overviewAgent(row, freshness: freshness, now: now)
        }
        return (list, exceptions.count - shownExceptions.count)
    }

    private static func overviewAgent(_ row: AgentRow, freshness: ProjectFreshness, now: Date) -> OverviewAgent {
        let key = row.source.key
        return OverviewAgent(
            id: key, agentId: row.agentId, role: row.role, health: row.health, healthWord: row.healthWord,
            healthSymbol: row.healthSymbol, task: row.task.map { (task: String) -> String in isolate(task) },
            taskUnverified: row.taskUnverified, taskSource: "task of " + row.agentId, branch: row.branch,
            workingFor: row.health == .working ? age(since: row.source.activitySince, now: now) : nil,
            isPending: freshness.pendingKeys.contains(FreshnessMarks.pendingKey(section: "agents", key: key)))
    }

    /// `now`, `6m`, `2h 5m`, `3d`; nil when the time is absent, unreadable or in the future (clock skew).
    static func age(since: String?, now: Date) -> String? {
        guard let start = OverviewTimes.date(since), start <= now else { return nil }
        let seconds = Int(now.timeIntervalSince(start))
        if seconds < 60 { return "now" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes)m" }
        let hours = minutes / 60
        if hours < 24 {
            let rest = minutes % 60
            return rest == 0 ? "\(hours)h" : "\(hours)h \(rest)m"
        }
        return "\(hours / 24)d"
    }

    /// Wraps agent text in a bidi isolate. The text itself was cleaned of control and override characters.
    static func isolate(_ text: String) -> String {
        "\u{2068}" + text + "\u{2069}"
    }

    // MARK: Plans

    private static func planList(_ project: ProjectModel, freshness: ProjectFreshness) -> [OverviewPlan] {
        let stored: [StoredPlan] = project.plans.values.filter { (plan: StoredPlan) -> Bool in
            planStates.contains(plan.state)
        }.sorted { (left: StoredPlan, right: StoredPlan) -> Bool in
            left.sequence == right.sequence ? left.planId < right.planId : left.sequence < right.sequence
        }
        return stored.map { (plan: StoredPlan) -> OverviewPlan in
            let row = PlanRow(plan)
            return OverviewPlan(
                id: plan.key, planId: row.planId, sequence: row.sequence,
                title: row.titleUnverified ? row.title : isolate(row.title), titleUnverified: row.titleUnverified,
                progress: row.progress, progressLabel: "\(row.integrated) of \(row.total) integrated",
                factLine: factLine(row),
                isPending: freshness.pendingKeys.contains(FreshnessMarks.pendingKey(section: "plans", key: plan.key)))
        }
    }

    /// Built from the state and the counts only.
    static func factLine(_ row: PlanRow) -> String {
        switch row.state {
        case "signed_off": return "signed off, not merged"
        case "submitted": return "submitted for approval"
        case "in_review": return "in review"
        default: break
        }
        guard row.total > 0 else { return "\(row.stateWord), no packages" }
        var parts: [String] = []
        if row.notStarted > 0 { parts.append("\(row.notStarted) unassigned") }
        if row.assignedOnly > 0 { parts.append("\(row.assignedOnly) assigned") }
        if row.reportedOnly > 0 { parts.append("\(row.reportedOnly) reported, not integrated") }
        parts.append("\(row.integrated) of \(row.total) integrated")
        return parts.joined(separator: " · ")
    }

    // MARK: Outcomes

    private static func outcome(_ entry: HistoryEvent, times: OverviewTimes) -> OverviewOutcome {
        let event = entry.event
        let host = times.hostTime(event.ts)
        return OverviewOutcome(
            id: "\(entry.key.epoch):\(entry.key.seq):\(entry.key.k)", seq: event.seq, k: event.k, type: event.type,
            symbol: symbol(event.type), text: outcomeText(event), hostTime: host.text, isSkewed: host.skewed,
            receivedTime: times.text(entry.receivedAt), isLive: entry.source == .live)
    }

    private static func symbol(_ type: String) -> String {
        switch type {
        case "report.recorded": return "arrow.up.circle"
        case "review.verdict": return "checkmark.seal"
        case "verify.result": return "checkmark.circle"
        case "integration.finished": return "arrow.triangle.merge"
        case "plan.approved", "plan.signed_off": return "doc.badge.checkmark"
        case "design.decided": return "paintbrush"
        case "agent.lost", "agent.blocked": return "exclamationmark.triangle"
        default: return "flag"
        }
    }

    /// A sentence of ids and states; the free text of an event is not part of it.
    static func outcomeText(_ event: FeedEvent) -> String {
        let entity = HostText.name(event.entity.id)
        let actor = HostText.name(event.actorId ?? event.entity.id)
        let to = event.to.map { (value: String) -> String in HostText.name(value) }
        switch event.type {
        case "report.recorded":
            var sha = ""
            if case .string(let full)? = event.body.raw["commitSha"] { sha = String(HostText.name(full).prefix(7)) }
            if case .string(let agent)? = event.body.raw["agentId"] {
                return HostText.name(agent) + " reported" + (sha.isEmpty ? "" : " " + sha)
            }
            return actor + " reported" + (sha.isEmpty ? "" : " " + sha)
        case "review.verdict":
            var verdict = to ?? "reviewed"
            if case .string(let word)? = event.body.raw["verdict"] { verdict = HostText.name(word) }
            return "review " + entity + " " + verdict
        case "verify.result":
            var result = to ?? "finished"
            if case .string(let word)? = event.body.raw["result"] { result = HostText.name(word) }
            return "verify " + result
        case "integration.finished":
            return "integration " + entity + " " + (to ?? "finished")
        case "plan.approved": return entity + " approved"
        case "plan.signed_off": return entity + " signed off"
        case "design.decided":
            if case .string(let word)? = event.body.raw["decision"] { return "design " + HostText.name(word) + " for " + entity }
            return "design decided for " + entity
        case "agent.lost": return entity + " lost"
        case "agent.blocked": return entity + " blocked"
        case "finding.raised": return "finding raised on " + entity
        default: return HostText.name(event.type)
        }
    }

    private struct HistoryParts {
        let kind: OverviewHistoryKind
        let text: String?
        let gap: String?
        let empty: String?
        let canRetry: Bool
    }

    private static func historyTexts(
        _ history: RecentHistoryState, outcomes: [OverviewOutcome], times: OverviewTimes, now: Date
    ) -> HistoryParts {
        var since: String?
        if let connected = history.connectedAt { since = "since you connected at " + times.text(connected) }
        let sinceEmpty = "No outcomes " + (since ?? "yet")
        var gap: String?
        if let gapStart = history.gapSince { gap = "History before " + times.text(gapStart) + " may be missing" }
        switch history.phase {
        case .idle:
            return HistoryParts(kind: .idle, text: nil, gap: gap, empty: outcomes.isEmpty ? sinceEmpty : nil, canRetry: false)
        case .loading:
            return HistoryParts(kind: .loading, text: "Loading history…", gap: gap, empty: nil, canRetry: false)
        case .sessionOnly:
            var note = "This host can't show earlier history."
            if let since { note += " Showing what happened " + since + "." }
            return HistoryParts(kind: .sessionOnly, text: note, gap: gap, empty: outcomes.isEmpty ? sinceEmpty : nil, canRetry: false)
        case .error(let message):
            var ready = true
            if let notBefore = history.retryNotBefore { ready = now >= notBefore }
            return HistoryParts(kind: .error, text: message, gap: gap, empty: outcomes.isEmpty ? sinceEmpty : nil, canRetry: ready)
        case .backfilled:
            if history.truncated {
                let empty = outcomes.isEmpty ? "No outcomes in the last \(scanLimitRows) events" : nil
                return HistoryParts(
                    kind: .truncated, text: "Earlier history not loaded: the host stopped after its scan limit", gap: gap,
                    empty: empty, canRetry: false)
            }
            let shown = min(outcomes.count, collapsedOutcomeCount)
            let note: String? = shown == 0 ? nil : (shown == 1 ? "Last outcome" : "Last \(shown) outcomes")
            return HistoryParts(
                kind: .backfilled, text: note, gap: gap, empty: outcomes.isEmpty ? "No outcomes recorded yet" : nil,
                canRetry: false)
        }
    }
}

/// Times as the overview words them: in the given calendar, with the day named only for yesterday.
struct OverviewTimes {
    let now: Date
    let calendar: Calendar

    /// A time on this Mac's clock: `14:05`, `yesterday 18:40`, `3 Oct 09:00`.
    func text(_ date: Date) -> String {
        let clock = format("HH:mm", date)
        if calendar.isDate(date, inSameDayAs: now) { return clock }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now),
           calendar.isDate(date, inSameDayAs: yesterday) {
            return "yesterday " + clock
        }
        return format("d MMM HH:mm", date)
    }

    /// The host's time of an event. A time ahead of now (clock skew) is the absolute time with no age.
    func hostTime(_ ts: String) -> (text: String, skewed: Bool) {
        guard let date = OverviewTimes.date(ts) else { return ("time unknown", false) }
        if date > now {
            let clock = format("HH:mm", date)
            return (calendar.isDate(date, inSameDayAs: now) ? clock : format("d MMM HH:mm", date), true)
        }
        if now.timeIntervalSince(date) < 60 { return ("now", false) }
        return (text(date), false)
    }

    private func format(_ pattern: String, _ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.dateFormat = pattern
        return formatter.string(from: date)
    }

    /// An ISO 8601 time with or without fractional seconds; nil when absent or unreadable.
    static func date(_ text: String?) -> Date? {
        guard let text else { return nil }
        return MenuList.date(text)
    }
}
