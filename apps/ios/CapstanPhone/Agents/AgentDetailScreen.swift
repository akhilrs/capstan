import CapstanAppCore
import CapstanKit
import SwiftUI

/// An agent's details (board 07): its health with what it means and what it rests on, the facts (task, branch,
/// state, generation), and Pause… or Resume. Pause confirms in a sheet at the medium detent with an optional
/// reason and Cancel as the default. While the whole project is paused, Resume is off and says why.
struct AgentDetailScreen: View {
    @Environment(AppModel.self) private var model
    let hostId: UUID
    let projectId: String
    let agentId: String

    @State private var confirmPause = false
    @State private var sending = false
    @State private var outcome: DecideOutcome?
    @State private var receivedAt = Date()

    var body: some View {
        if let project = model.project(hostId, projectId),
            let stored = project.agents.values.first(where: { $0.agentId == agentId })
        {
            content(AgentRow(agent: stored, in: project), stored, project)
        } else {
            ContentUnavailableView("This agent is no longer listed", systemImage: "person.2")
                .background(Tokens.color(.ground))
        }
    }

    private var hostName: String {
        model.hosts.first { $0.id == hostId }?.displayName ?? "the host"
    }

    private func content(_ row: AgentRow, _ stored: StoredAgent, _ project: ProjectModel) -> some View {
        let projectName = TextBudget.displayPrefix(project.displayName, maxBytes: 64).text
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: stored.requestId)
        let control = PauseControl(
            capabilities: model.capabilities[hostId], busy: model.busy.keys, key: key, paused: stored.paused)
        let runPaused = project.run?.paused ?? false
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: "person.2").foregroundStyle(Tokens.color(.accent)).accessibilityHidden(true)
                        Text(verbatim: "Agent · " + projectName).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                    }
                    Text(verbatim: row.agentId + " · " + row.role).font(TypeStyle.title2).accessibilityAddTraits(.isHeader)
                    StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health))
                }
                if let outcome, !sending {
                    OutcomeBanner(outcome: outcome, hostName: hostName, receivedAt: receivedAt)
                }
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    Text(verbatim: AgentDetailScreen.explanation(row.health))
                        .font(TypeStyle.subheadline)
                        .fixedSize(horizontal: false, vertical: true)
                    if let basis = AgentHealth.basis(of: stored, in: project).line(now: Date()) {
                        Text(verbatim: basis).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    }
                }
                DetailFacts(facts: facts(row, stored))
                if let task = stored.task {
                    DetailTextBlock(text: task, name: "Task")
                }
                if row.paused, runPaused {
                    DetailNotice(
                        tone: .grey, symbol: "pause", title: "The whole project is paused",
                        text: "Resume is off until " + projectName + " resumes. Open the project to resume it.")
                }
                if model.capabilities[hostId]?.decide == false {
                    DetailNotice(
                        tone: .grey, symbol: "apple.terminal", title: "Pause and resume in the terminal",
                        text: "This host's Capstan has no remote decisions.")
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: Tokens.space[2]) {
                if stored.paused {
                    Button { send(.agentResume(reason: nil)) } label: {
                        if sending { SendingLabel() } else { Text("Resume") }
                    }
                    .buttonStyle(.capstanPrimary)
                    .disabled(sending || runPaused || !control.resume.isEnabled)
                    Button { openProject() } label: { Text("Open project") }
                        .buttonStyle(.capstanQuiet)
                } else {
                    Button { confirmPause = true } label: {
                        if sending { SendingLabel() } else { Text("Pause…") }
                    }
                    .buttonStyle(.capstanDestructive)
                    .disabled(sending || !control.pause.isEnabled)
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .background(Tokens.color(.ground))
        }
        .sheet(isPresented: $confirmPause) {
            ConfirmSheet(
                title: "Pause " + row.agentId + "?", detail: nil, noteLimit: ProtocolLimits.reasonCap,
                destructiveLabel: "Pause " + row.agentId, noteLabel: "Reason (optional)",
                footnote: row.agentId + " stops after its current step and keeps its task and branch. Resume it from here or with cstan resume on the host.",
                onConfirm: { reason in send(.agentPause(reason: reason)) })
        }
    }

    private func facts(_ row: AgentRow, _ stored: StoredAgent) -> [DetailFact] {
        var rows: [DetailFact] = []
        if let branch = row.branch { rows.append(DetailFact(label: "Branch", value: branch, mono: true)) }
        rows.append(DetailFact(label: "State", value: row.state + (stored.paused ? ", paused" : "")))
        rows.append(DetailFact(label: "Generation", value: String(stored.generation)))
        rows.append(DetailFact(label: "Kind", value: TextBudget.displayPrefix(stored.kind, maxBytes: 64).text))
        return rows
    }

    private func send(_ verb: DecideVerb) {
        guard !sending else { return }
        sending = true
        outcome = nil
        Task {
            let result = await model.decideAgent(verb, hostId: hostId, projectId: projectId, agentId: agentId)
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

    private func openProject() {
        let tab = model.navigation.tab
        model.navigation.show(tab, model.navigation.path(tab) + [.project(hostId: hostId, projectId: projectId)])
    }

    static func explanation(_ health: AgentHealth) -> String {
        switch health {
        case .working:
            return "The host sees the agent busy in its terminal. Without the host's activity, Working is an estimate from its state and task; it does not say the agent made progress."
        case .needsYou:
            return "A prompt from this agent waits for your answer in Waiting, or the host sees it stopped at a question in its terminal. If Waiting has no prompt from it, answer in the agent's terminal."
        case .stalled:
            return "The controller saw no progress for longer than its limit. The connection is fine. You can pause the agent while you look, or leave it to the supervisor."
        case .lost:
            return "The controller marked the agent lost: its pane is gone and the controller will not hear from it again."
        case .blocked:
            return "The agent told the controller it cannot go on without something from outside."
        case .paused:
            return "The agent was paused. Resume it when you are ready."
        case .idle:
            return "The host sees the agent waiting at its prompt or done with its turn. Without the host's activity, Idle is an estimate."
        }
    }
}

#Preview("Agents") {
    NavigationStack { AgentsScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Agent, stalled") {
    NavigationStack { AgentDetailScreen(hostId: PreviewData.alphaId, projectId: "capstan", agentId: "dev-stalled") }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Agent, paused") {
    NavigationStack { AgentDetailScreen(hostId: PreviewData.alphaId, projectId: "capstan", agentId: "dev-paused") }
        .environment(AppModel(preview: PreviewData.standard))
}
