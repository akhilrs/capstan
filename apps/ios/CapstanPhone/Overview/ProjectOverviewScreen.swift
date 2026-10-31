import CapstanAppCore
import CapstanKit
import SwiftUI

/// A ready project (board 09), reached from Hosts: the status word with the agent counts and how current it is, what
/// waits, the agents now, the plans, and recent outcomes in host time. The "…" menu opens plans and agents, refreshes
/// the history, and pauses or resumes the whole project; Pause project… confirms in a sheet with Cancel first.
struct ProjectOverviewScreen: View {
    @Environment(AppModel.self) private var model
    let hostId: UUID
    let projectId: String

    @State private var confirmPause = false
    @State private var sending = false
    @State private var outcome: DecideOutcome?
    @State private var receivedAt = Date()

    private static let runKey = "run"

    var body: some View {
        Group {
            if let project = model.project(hostId, projectId),
                let content = model.overviewContent(hostId: hostId, projectId: projectId)
            {
                screen(project, content)
            } else {
                ContentUnavailableView("This project is no longer listed", systemImage: "folder")
            }
        }
        .background(Tokens.color(.ground))
        .onAppear { model.overviewShow(hostId: hostId, projectId: projectId) }
        .onDisappear {
            if model.overview.shownProject == OverviewStore.ShownProject(hostId: hostId, projectId: projectId) {
                model.overviewShow(hostId: hostId, projectId: nil)
            }
        }
        .onChange(of: model.project(hostId, projectId)?.head) { _, _ in
            model.overviewFeed(hostId: hostId, projectId: projectId)
        }
    }

    private var hostName: String {
        model.hosts.first { $0.id == hostId }?.displayName ?? "the host"
    }

    private func screen(_ project: ProjectModel, _ content: OverviewModel) -> some View {
        let name = TextBudget.displayPrefix(project.displayName, maxBytes: 64).text
        let runPaused = project.run?.paused ?? false
        let waiting = project.waiting.values.sorted { $0.since < $1.since }
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: name).font(TypeStyle.largeTitle).lineLimit(2).accessibilityAddTraits(.isHeader)
                    Text(verbatim: hostName + " · " + project.root)
                        .font(TypeStyle.monoSmall)
                        .foregroundStyle(Tokens.color(.label2))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                statusCard(content)
                if let outcome, !sending {
                    OutcomeBanner(outcome: outcome, hostName: hostName, receivedAt: receivedAt)
                }
                if !waiting.isEmpty {
                    section("Waiting for you", detail: String(waiting.count)) {
                        ForEach(Array(waiting.enumerated()), id: \.element.key) { index, item in
                            if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                            NavigationLink(value: PhoneRoute.item(InboxRowID(hostId: hostId, projectId: projectId, key: item.key))) {
                                WaitingLine(item: item)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                agentsSection(content)
                if !content.plans.isEmpty {
                    section("Plans", detail: String(content.plans.count)) {
                        ForEach(Array(content.plans.enumerated()), id: \.element.id) { index, plan in
                            if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                            NavigationLink(value: PhoneRoute.plan(PlanRef(hostId: hostId, projectId: projectId, planId: plan.planId))) {
                                OverviewPlanLine(plan: plan)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                outcomesSection(content)
                if runPaused {
                    DetailNotice(
                        tone: .grey, symbol: "pause", title: "The whole project is paused",
                        text: "No agent works until it resumes. Resume it from the … menu or with cstan resume on the host.")
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .refreshable {
            model.overviewRefreshed()
            model.refresh(hostId: hostId, projectIds: [projectId])
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .topBarTrailing) { menu(project, runPaused: runPaused) } }
        .sheet(isPresented: $confirmPause) {
            ConfirmSheet(
                title: "Pause " + name + "?", detail: nil, noteLimit: ProtocolLimits.reasonCap,
                destructiveLabel: "Pause project", noteLabel: "Reason (optional)",
                footnote: "Every agent of " + name + " stops after its current step and keeps its task and branch. Resume the project from here or with cstan resume on the host.",
                onConfirm: { reason in send(.runPause(reason: reason)) })
        }
    }

    // MARK: Sections

    private func statusCard(_ content: OverviewModel) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[2]) {
                StatePill(word: content.statusWord, symbol: Self.symbol(content.status), tone: Self.tone(content.status))
                    .fixedSize()
                Text(verbatim: content.countsText).font(TypeStyle.subheadline)
            }
            Text(verbatim: content.freshnessLine).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            if let fact = content.quietFact {
                Text(verbatim: fact).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(Tokens.space[3])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        .accessibilityElement(children: .combine)
        .accessibilityValue(Text(verbatim: content.accessibilityValue))
    }

    @ViewBuilder
    private func agentsSection(_ content: OverviewModel) -> some View {
        if content.totalAgents == 0 {
            section("Agents now", detail: nil) {
                Text(verbatim: content.agentsEmptyText)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .padding(Tokens.space[3])
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            section("Agents now", detail: String(content.agentsNow.count) + " of " + String(content.totalAgents)) {
                ForEach(Array(content.agentsNow.enumerated()), id: \.element.id) { index, agent in
                    if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                    NavigationLink(value: PhoneRoute.agent(hostId: hostId, projectId: projectId, agentId: agent.agentId)) {
                        OverviewAgentLine(agent: agent)
                    }
                    .buttonStyle(.plain)
                }
                if let more = content.moreNeedAttentionText {
                    Divider().padding(.leading, Tokens.space[3])
                    Text(verbatim: more)
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                        .padding(Tokens.space[3])
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                Divider().padding(.leading, Tokens.space[3])
                Button { model.navigation.show(.agents, []) } label: {
                    HStack {
                        Text(verbatim: content.viewAllText ?? "View all agents").foregroundStyle(Tokens.color(.label))
                        Spacer()
                        Image(systemName: "chevron.right")
                            .font(TypeStyle.footnote.weight(.semibold))
                            .foregroundStyle(Tokens.color(.label2))
                            .accessibilityHidden(true)
                    }
                    .padding(.horizontal, Tokens.space[3])
                    .frame(minHeight: 52)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func outcomesSection(_ content: OverviewModel) -> some View {
        section("Recent outcomes", detail: "host time") {
            if content.historyState == .loading && content.outcomes.isEmpty {
                HStack(spacing: Tokens.space[2]) {
                    ProgressView()
                    Text("Reading the history…").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                }
                .padding(Tokens.space[3])
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            ForEach(Array(content.outcomes.enumerated()), id: \.element.id) { index, outcome in
                if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                OutcomeLine(outcome: outcome)
            }
            if let empty = content.outcomesEmptyText, content.outcomes.isEmpty, content.historyState != .loading {
                note(empty)
            }
            if let gap = content.historyGapText { note(gap) }
            if let text = content.historyText { note(text) }
            if content.canRetryHistory {
                Button("Retry") { model.overviewRefreshed() }
                    .buttonStyle(.capstanQuiet)
                    .fixedSize()
                    .padding([.horizontal, .bottom], Tokens.space[3])
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(verbatim: text)
            .font(TypeStyle.footnote)
            .foregroundStyle(Tokens.color(.label2))
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func section<Content: View>(
        _ title: String, detail: String?, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: title).font(TypeStyle.headline)
                if let detail {
                    Text(verbatim: detail).font(TypeStyle.headline.weight(.regular)).foregroundStyle(Tokens.color(.label2))
                }
            }
            .accessibilityAddTraits(.isHeader)
            VStack(spacing: 0) { content() }
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        }
    }

    // MARK: Menu and pause

    private func menu(_ project: ProjectModel, runPaused: Bool) -> some View {
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: project.run?.requestId ?? Self.runKey)
        let control = ProjectPauseControl(
            capabilities: model.capabilities[hostId], busy: model.busy.keys, key: key, runPaused: runPaused,
            hashKnown: project.run?.hashKnown ?? true)
        return Menu {
            Button { model.navigation.show(.plans, []) } label: { Label("Open plans", systemImage: "list.bullet.rectangle") }
            Button { model.navigation.show(.agents, []) } label: { Label("Open agents", systemImage: "person.2") }
            Button {
                model.overviewRefreshed()
                model.refresh(hostId: hostId, projectIds: [projectId])
            } label: {
                Label("Refresh history", systemImage: "arrow.clockwise")
            }
            Divider()
            if runPaused {
                Button { send(.runResume(reason: nil)) } label: { Label("Resume project", systemImage: "play") }
                    .disabled(sending || !control.state.isEnabled)
            } else {
                Button(role: .destructive) { confirmPause = true } label: { Label("Pause project…", systemImage: "pause") }
                    .disabled(sending || !control.state.isEnabled)
            }
        } label: {
            Label("More actions", systemImage: "ellipsis")
        }
    }

    private func send(_ verb: DecideVerb) {
        guard !sending else { return }
        sending = true
        outcome = nil
        Task {
            let result = await model.decideRun(verb, hostId: hostId, projectId: projectId)
            receivedAt = Date()
            outcome = result
            sending = false
            switch OutcomeLines.feel(result) {
            case .success?: Haptics.play(.success)
            case .warning?: Haptics.play(.warning)
            case .error?: Haptics.play(.error)
            case nil: break
            }
        }
    }

    static func tone(_ status: OverviewStatus) -> Tone {
        switch status {
        case .moving: return .green
        case .needsAttention: return .yellow
        case .paused, .quiet, .unknown: return .grey
        }
    }

    static func symbol(_ status: OverviewStatus) -> String {
        switch status {
        case .moving: return "checkmark"
        case .needsAttention: return "exclamationmark.triangle"
        case .paused: return "pause"
        case .quiet: return "moon"
        case .unknown: return "questionmark.circle"
        }
    }
}

/// One waiting item of the project: its kind glyph and name, its age, and its summary as plain text.
private struct WaitingLine: View {
    let item: StoredWaiting

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            Image(systemName: item.kind.glyph).foregroundStyle(Tokens.color(.label2)).frame(width: 24).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack {
                    Text(verbatim: RowText.kindName(item.kind)).font(TypeStyle.headline)
                    Spacer()
                    Text(verbatim: AgeText.short(since: item.since, now: Date()))
                        .font(TypeStyle.subheadline.monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                }
                if item.summary.verified {
                    Text(verbatim: TextBudget.displayPrefix(item.summary.displayPlain, maxBytes: 256).text)
                        .font(TypeStyle.monoSmall)
                        .foregroundStyle(Tokens.color(.label2))
                        .lineLimit(2)
                } else {
                    Text("Text could not be verified").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                }
            }
            Image(systemName: "chevron.right")
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
                .accessibilityHidden(true)
        }
        .padding(Tokens.space[3])
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// One agent under "Agents now": id, role, health pill with how long it has worked, and its task.
private struct OverviewAgentLine: View {
    let agent: OverviewAgent

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                    Text(verbatim: agent.agentId).font(TypeStyle.headline)
                    Text(verbatim: agent.role).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    Spacer(minLength: Tokens.space[1])
                    StatePill(
                        word: agent.workingFor.map { agent.healthWord + " " + $0 } ?? agent.healthWord,
                        symbol: agent.healthSymbol, tone: AgentTone.tone(agent.health))
                        .fixedSize()
                }
                Text(verbatim: agent.task ?? (agent.taskUnverified ? "Task not shown: it failed its integrity check" : "No task"))
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(2)
            }
            Image(systemName: "chevron.right")
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
                .accessibilityHidden(true)
        }
        .padding(Tokens.space[3])
        .opacity(agent.isPending ? 0.6 : 1)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// One plan of the project: title, bar and fact line.
private struct OverviewPlanLine: View {
    let plan: OverviewPlan

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: plan.title).font(TypeStyle.headline).lineLimit(2)
                Spacer()
                Text(verbatim: plan.planId).font(TypeStyle.monoSmall).foregroundStyle(Tokens.color(.label2))
            }
            PlanProgressBar(progress: plan.progress).accessibilityHidden(true)
            Text(verbatim: plan.progressLabel + " · " + plan.factLine)
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
        }
        .padding(Tokens.space[3])
        .opacity(plan.isPending ? 0.6 : 1)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// One outcome in host time: the time in mono, what happened (ids and states only), and its glyph.
private struct OutcomeLine: View {
    let outcome: OverviewOutcome

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[2]) {
            Text(verbatim: outcome.hostTime).font(TypeStyle.monoSmall).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: outcome.text).font(TypeStyle.subheadline).frame(maxWidth: .infinity, alignment: .leading)
            Image(systemName: outcome.symbol).foregroundStyle(Tokens.color(.label2)).accessibilityHidden(true)
        }
        .padding(Tokens.space[3])
        .accessibilityElement(children: .combine)
    }
}

#Preview("Project overview") {
    NavigationStack { ProjectOverviewScreen(hostId: PreviewData.alphaId, projectId: "capstan").routes() }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Project overview, dark") {
    NavigationStack { ProjectOverviewScreen(hostId: PreviewData.alphaId, projectId: "capstan").routes() }
        .environment(AppModel(preview: PreviewData.standard))
        .preferredColorScheme(.dark)
}
