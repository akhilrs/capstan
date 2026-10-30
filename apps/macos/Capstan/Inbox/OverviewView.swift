import CapstanAppCore
import CapstanKit
import SwiftUI

/// What the controls of the overview do. The defaults do nothing, for previews.
struct OverviewActions {
    var toggle: (String) -> Void = { _ in }
    var selectOutcome: (String) -> Void = { _ in }
    var retryHistory: () -> Void = {}
    var viewAllAgents: () -> Void = {}
    var refresh: () -> Void = {}
}

/// The list column of a selected project: the status strip, the waiting groups when any wait, and under them the
/// sections Agents now, Plans in progress and Recent outcomes. When items wait, the sections sit under a Project
/// divider.
struct OverviewView<Waiting: View>: View {
    let content: OverviewModel
    let projectName: String
    let anythingWaiting: Bool
    let selectedOutcome: String?
    let actions: OverviewActions
    private let waiting: Waiting

    init(
        content: OverviewModel, projectName: String, anythingWaiting: Bool, selectedOutcome: String? = nil,
        actions: OverviewActions = OverviewActions(), @ViewBuilder waiting: () -> Waiting
    ) {
        self.content = content
        self.projectName = projectName
        self.anythingWaiting = anythingWaiting
        self.selectedOutcome = selectedOutcome
        self.actions = actions
        self.waiting = waiting()
    }

    var body: some View {
        List {
            StatusStripView(content: content, onRefresh: actions.refresh)
                .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 8, trailing: 12))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            waiting
            if anythingWaiting {
                Section {
                    sections
                } header: {
                    ProjectDivider(name: projectName)
                }
            } else {
                Section {
                    sections
                }
            }
            // Keeps the last line above the floating status capsule.
            Color.clear.frame(height: StatusCapsule.reservedHeight)
                .listRowInsets(EdgeInsets()).listRowBackground(Color.clear).listRowSeparator(.hidden)
                .accessibilityHidden(true)
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
    }

    private var sections: some View {
        OverviewSections(content: content, selectedOutcome: selectedOutcome, actions: actions)
            .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 4, trailing: 12))
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
    }
}

/// The line that sets the project's sections apart from the waiting groups above them.
private struct ProjectDivider: View {
    let name: String

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Divider()
            Text(verbatim: "Project · " + name)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(1)
                .accessibilityAddTraits(.isHeader)
        }
        .padding(.top, Tokens.space[1])
    }
}

/// The three foldable sections. Rows keep their identity (the agent's key, the plan's key, the outcome's key), so a
/// reorder moves a row instead of redrawing it. Last-known values are dimmed and an item that waits for its re-read
/// is dimmed alone.
struct OverviewSections: View {
    static let dimming = 0.55

    let content: OverviewModel
    let selectedOutcome: String?
    let actions: OverviewActions

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            SectionBlock(
                title: "Agents now", symbol: "person.2", folded: content.isFolded(OverviewModel.agentsFolded),
                onToggle: { actions.toggle(OverviewModel.agentsFolded) }
            ) {
                agents
            }
            SectionBlock(
                title: "Plans in progress", symbol: "doc.text", folded: content.isFolded(OverviewModel.plansFolded),
                onToggle: { actions.toggle(OverviewModel.plansFolded) }
            ) {
                plans
            }
            SectionBlock(
                title: "Recent outcomes", symbol: "flag", folded: content.isFolded(OverviewModel.outcomesFolded),
                onToggle: { actions.toggle(OverviewModel.outcomesFolded) }
            ) {
                RecentOutcomesView(
                    content: content, selectedOutcome: selectedOutcome, onSelect: actions.selectOutcome,
                    onRetry: actions.retryHistory)
            }
        }
        .opacity(content.isLastKnown ? OverviewSections.dimming : 1)
    }

    @ViewBuilder
    private var agents: some View {
        if content.agentsNow.isEmpty {
            Text(verbatim: content.agentsEmptyText).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
        }
        ForEach(content.agentsNow) { agent in
            AgentLine(agent: agent)
        }
        if let more = content.moreNeedAttentionText {
            Text(verbatim: more).font(.system(size: 12, weight: .medium)).foregroundStyle(Tokens.color(.yellow))
        }
        if let viewAll = content.viewAllText {
            Button(action: actions.viewAllAgents) { Text(verbatim: viewAll) }
                .buttonStyle(.capstanQuiet)
        }
    }

    @ViewBuilder
    private var plans: some View {
        if content.plans.isEmpty {
            Text(verbatim: content.plansEmptyText).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
        }
        ForEach(content.plans) { plan in
            PlanLine(plan: plan)
        }
    }
}

/// One foldable section: a header button with a chevron, and the body below it when open.
private struct SectionBlock<Inner: View>: View {
    let title: String
    let symbol: String
    let folded: Bool
    let onToggle: () -> Void
    private let inner: Inner

    init(
        title: String, symbol: String, folded: Bool, onToggle: @escaping () -> Void,
        @ViewBuilder inner: () -> Inner
    ) {
        self.title = title
        self.symbol = symbol
        self.folded = folded
        self.onToggle = onToggle
        self.inner = inner()
    }

    var body: some View {
        let state: String = folded ? "Folded" : "Open"
        return VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Button(action: onToggle) {
                HStack(spacing: Tokens.space[0] + 2) {
                    Image(systemName: folded ? "chevron.right" : "chevron.down").imageScale(.small).frame(width: 12)
                    Image(systemName: symbol).imageScale(.small)
                    Text(verbatim: title).font(.system(size: 12, weight: .semibold))
                    Spacer(minLength: 0)
                }
                .foregroundStyle(Tokens.color(.label2))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(state)
            if !folded {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    inner
                }
                .padding(.leading, Tokens.space[3])
            }
        }
    }
}

private struct AgentLine: View {
    let agent: OverviewAgent

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
            StatePill(word: agent.healthWord, symbol: agent.healthSymbol, tone: AgentTone.tone(agent.health))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: Tokens.space[0]) {
                    Text(verbatim: agent.agentId + " · " + agent.role).font(.system(size: 13, weight: .semibold))
                    if agent.isPending {
                        Text(verbatim: "updating").font(.system(size: 11)).foregroundStyle(Tokens.color(.label2))
                    }
                }
                task
                if let branch = agent.branch {
                    Text(verbatim: branch)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Tokens.color(.label2))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: Tokens.space[0])
            if let workingFor = agent.workingFor {
                Text(verbatim: workingFor)
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(Tokens.color(.label2))
            }
        }
        .opacity(agent.isPending ? OverviewSections.dimming : 1)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var task: some View {
        if let task = agent.task {
            Text(verbatim: task)
                .font(.system(size: 13))
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Text(verbatim: agent.taskSource).font(.system(size: 11)).foregroundStyle(Tokens.color(.label2))
        } else if agent.taskUnverified {
            Text("Task not shown: the host could not vouch for it")
                .font(.system(size: 12))
                .foregroundStyle(Tokens.color(.label2))
        }
    }
}

private struct PlanLine: View {
    let plan: OverviewPlan

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(spacing: Tokens.space[0]) {
                Text(verbatim: "plan-\(plan.sequence) · " + plan.title)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                if plan.isPending {
                    Text(verbatim: "updating").font(.system(size: 11)).foregroundStyle(Tokens.color(.label2))
                }
            }
            PlanProgressBar(progress: plan.progress)
            Text(verbatim: plan.progressLabel + " · " + plan.factLine)
                .font(.system(size: 12))
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
        }
        .opacity(plan.isPending ? OverviewSections.dimming : 1)
        .accessibilityElement(children: .combine)
    }
}

#if DEBUG
/// Data for the previews of the overview, built here so the previews depend on no other fixture.
enum OverviewSamples {
    enum HistoryKind {
        case loading, backfilled, sessionOnly, error, truncated, gap
    }

    static let projectId = "capstan"
    static let epoch = "daeca3b2245f3519"
    static let now: Date = ISO8601DateFormatter().date(from: "2026-10-29T14:05:00Z") ?? Date(timeIntervalSince1970: 1_793_282_700)
    static let connectedAt: Date = now.addingTimeInterval(-3_600)

    static var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? TimeZone.current
        return calendar
    }

    private static func text(_ raw: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: false)
    }

    private static func agent(
        _ id: String, role: String, state: String = "active", task: String?, activity: String? = "working",
        since: String? = "2026-10-29T13:59:00Z", branch: String? = nil
    ) -> StoredAgent {
        StoredAgent(
            key: "agent:" + id, agentId: id, kind: "claude", role: role, generation: 1, state: state, paused: false,
            task: task.map { (raw: String) -> UntrustedText in text(raw) }, requestId: "req-" + id,
            contentHash: "h-" + id, branch: branch.map { (raw: String) -> UntrustedText in text(raw) },
            activity: activity, activitySince: since, activitySeq: 7)
    }

    private static func plan(_ number: Int64, _ title: String, state: String, counts: StoredCounts) -> StoredPlan {
        StoredPlan(
            key: "plan:plan-\(number)", planId: "plan-\(number)", sequence: number, title: text(title), state: state,
            packages: counts)
    }

    static func project(
        agents: Bool = true, plans: Bool = true, folded: [String]? = nil, freshness: FreshnessMarks? = nil
    ) -> ProjectModel {
        var agentList: [StoredAgent] = []
        if agents {
            agentList = [
                agent("dev-1", role: "developer", task: "Write the overview views", branch: "feat/plan-139-ui-overview"),
                agent("dev-2", role: "developer", task: "Write the controller tests", since: "2026-10-29T13:40:00Z"),
                agent("dev-3", role: "developer", task: "Wait for the build slot", activity: "idle", since: nil),
                agent("rev-1", role: "reviewer", state: "lost", task: nil, activity: nil, since: nil),
                agent("pm-1", role: "pm", task: "Plan the next wave", activity: "idle", since: nil),
            ]
        }
        var planList: [StoredPlan] = []
        if plans {
            planList = [
                plan(139, "Mac app: project overview", state: "approved", counts: StoredCounts(total: 6, assigned: 5, reported: 4, integrated: 3)),
                plan(140, "Recent outcomes from the host", state: "in_review", counts: StoredCounts(total: 0, assigned: 0, reported: 0, integrated: 0)),
            ]
        }
        return ProjectModel(
            projectId: projectId, root: "/work/capstan", availability: .ready, epoch: epoch, head: 42,
            agents: Dictionary(uniqueKeysWithValues: agentList.map { ($0.key, $0) }),
            plans: Dictionary(uniqueKeysWithValues: planList.map { ($0.key, $0) }),
            attention: ["dev-3": "stalled"], snapshotComplete: true, freshness: freshness, foldedSections: folded)
    }

    private static func event(
        _ seq: Int64, _ type: String, entity: String, to: String? = nil, time: String, body: [String: JSONValue] = [:]
    ) -> FeedEvent? {
        var fields = body
        fields["effects"] = .array([])
        var members: [String: JSONValue] = [
            "id": .string("capstan:" + epoch + ":\(seq)"), "seq": .int(seq), "k": .int(0), "ts": .string(time),
            "type": .string(type), "entity": .object(["type": .string("package"), "id": .string(entity)]),
            "body": .object(fields),
        ]
        if let to { members["to"] = .string(to) }
        return try? FeedEvent(json: .object(members))
    }

    /// Newest first.
    static var events: [FeedEvent] {
        let list: [FeedEvent?] = [
            event(41, "integration.finished", entity: "pkg-ui-overview", to: "merged", time: "2026-10-29T14:03:00Z"),
            event(39, "review.verdict", entity: "pkg-kit-overview", time: "2026-10-29T13:52:00Z", body: ["verdict": .string("passed")]),
            event(37, "verify.result", entity: "pkg-kit-overview", time: "2026-10-29T13:40:00Z", body: ["result": .string("passed")]),
            event(33, "report.recorded", entity: "pkg-kit-overview", time: "2026-10-29T13:30:00Z", body: ["agentId": .string("dev-2"), "commitSha": .string("4a1f55f1f8cd9892ffdfbff557f049fc3b7462a9")]),
            event(30, "plan.approved", entity: "plan-139", time: "2026-10-28T18:40:00Z"),
            event(28, "agent.lost", entity: "rev-1", time: "2026-10-28T17:10:00Z"),
            event(25, "design.decided", entity: "plan-139", time: "2026-10-28T16:00:00Z", body: ["decision": .string("approved")]),
        ]
        return list.compactMap { (item: FeedEvent?) -> FeedEvent? in item }
    }

    private static func page(truncated: Bool) -> RecentPage {
        RecentPage(projectId: projectId, epoch: epoch, head: 42, events: events, truncated: truncated)
    }

    static func history(_ kind: HistoryKind) -> RecentHistoryState {
        var state = RecentHistoryState(projectId: projectId)
        state.connected(at: connectedAt)
        switch kind {
        case .loading:
            state.begin(epoch: epoch, head: 42)
        case .backfilled:
            state.begin(epoch: epoch, head: 42)
            state.apply(.page(page(truncated: false)), at: now)
        case .truncated:
            state.begin(epoch: epoch, head: 42)
            state.apply(.page(page(truncated: true)), at: now)
        case .sessionOnly:
            state.apply(.sessionOnly, at: now)
            state.merge(live: Array(events.prefix(2)), epoch: epoch, at: now)
        case .error:
            state.begin(epoch: epoch, head: 42)
            state.apply(.error(code: "failed", message: "no answer"), at: now)
        case .gap:
            state.resync(at: now.addingTimeInterval(-300))
            state.apply(.sessionOnly, at: now)
            state.merge(live: Array(events.prefix(2)), epoch: epoch, at: now)
        }
        return state
    }

    static func freshness(
        liveness: ConnectionLiveness = .live, refreshing: Bool = false, pending: Set<String> = [],
        failed: Bool = false
    ) -> ProjectFreshness {
        ProjectFreshness(
            liveness: liveness, availability: .ready, isRefreshing: refreshing, pendingKeys: pending,
            currentAsOf: refreshing || failed ? nil : now.addingTimeInterval(-20),
            lastKnownAt: now.addingTimeInterval(-300), refreshFailedAt: failed ? now.addingTimeInterval(-10) : nil)
    }

    static func content(
        history kind: HistoryKind = .backfilled, freshness marks: ProjectFreshness = freshness(),
        folded: Set<String> = []
    ) -> OverviewModel {
        OverviewModel.make(
            project: project(), freshness: marks, history: history(kind), folded: folded, now: now,
            calendar: calendar)
    }

    /// The outcome the inspector preview shows.
    static var outcome: OverviewOutcome? {
        content().outcomes.first
    }
}

/// Two waiting groups for the previews, drawn as the inbox draws them.
private struct SampleWaiting: View {
    var body: some View {
        Section {
            ForEach(["git push origin feat/plan-139", "make deploy"], id: \.self) { command in
                HStack(spacing: Tokens.space[1]) {
                    Image(systemName: "terminal").frame(width: 20).foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: command).font(.system(size: 13, weight: .semibold))
                    Spacer(minLength: 0)
                }
                .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 4, trailing: 12))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
        } header: {
            KindGroupHeader(title: "Command proposals · 2")
        }
    }
}

private func samplePreview(
    _ content: OverviewModel, waiting: Bool = false, width: CGFloat = 560, height: CGFloat = 640
) -> some View {
    Group {
        if waiting {
            OverviewView(content: content, projectName: "capstan", anythingWaiting: true) { SampleWaiting() }
        } else {
            OverviewView(content: content, projectName: "capstan", anythingWaiting: false) { EmptyView() }
        }
    }
    .frame(width: width, height: height)
}

#Preview("Overview17NothingWaits") {
    samplePreview(OverviewSamples.content())
}

#Preview("Overview18StripAboveWaiting") {
    samplePreview(OverviewSamples.content(folded: [OverviewModel.plansFolded, OverviewModel.outcomesFolded]), waiting: true)
}

#Preview("Overview19Compact") {
    samplePreview(OverviewSamples.content(), waiting: true, width: 640, height: 480)
        .dynamicTypeSize(.xxxLarge)
}

#Preview("Overview20Refreshing") {
    samplePreview(OverviewSamples.content(history: .loading, freshness: OverviewSamples.freshness(refreshing: true)))
}

#Preview("Overview20ItemUpdating") {
    let pending: Set<String> = [FreshnessMarks.pendingKey(section: "agents", key: "agent:dev-1")]
    samplePreview(OverviewSamples.content(freshness: OverviewSamples.freshness(pending: pending)))
}

#Preview("Overview20RefreshFailed") {
    samplePreview(OverviewSamples.content(freshness: OverviewSamples.freshness(failed: true)))
}

#Preview("Overview20HostSilent") {
    samplePreview(OverviewSamples.content(freshness: OverviewSamples.freshness(liveness: .silent)))
}
#endif
