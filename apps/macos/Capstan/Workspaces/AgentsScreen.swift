import CapstanAppCore
import CapstanKit
import SwiftUI

/// The agents of a project as a table (agent, state, branch and task, health), with a bar for the whole project that
/// pauses and resumes it. Pausing one agent is in the inspector.
struct AgentsScreen: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String

    @State private var outcome: DecideOutcome?
    @State private var askingProjectPause = false

    private static let runKey = "run"

    private var project: ProjectModel? { model.project(hostId, projectId) }
    private var decide: HostCapabilities? { model.capabilities[hostId] }
    private var hostName: String {
        model.hosts.first { $0.id == hostId }.map(HostLabel.hostName) ?? "the host"
    }

    var body: some View {
        if let project {
            let all = AgentRow.rows(project)
            let rows = all.filter(model.agentFilter.matches)
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    projectBar(project, all)
                    if rows.isEmpty {
                        Text("No agents yet").foregroundStyle(Tokens.color(.label2))
                    } else {
                        table(rows)
                    }
                    // Keeps the last row above the floating status capsule.
                    Color.clear.frame(height: StatusCapsule.reservedHeight)
                }
                .padding(Tokens.space[3])
            }
            .sheet(isPresented: $askingProjectPause) {
                ConfirmSheet(
                    title: "Pause the project?", detail: nil, noteLimit: ProtocolLimits.reasonCap,
                    destructiveLabel: "Pause"
                ) { reason in
                    sendRun(.runPause(reason: reason))
                }
            }
        } else {
            Text("This project is not available").foregroundStyle(Tokens.color(.label2)).padding()
        }
    }

    // MARK: Project

    private func projectBar(_ project: ProjectModel, _ rows: [AgentRow]) -> some View {
        let runPaused = project.run?.paused ?? false
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: project.run?.requestId ?? Self.runKey)
        let control = ProjectPauseControl(
            capabilities: decide, busy: model.busy.keys, key: key, runPaused: runPaused)
        let paused = rows.filter { $0.paused }.count
        return HStack(spacing: Tokens.space[2]) {
            Image(systemName: runPaused ? "pause" : "play").foregroundStyle(Tokens.color(.label2))
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: HostLabel.projectName(project) + (runPaused ? " is paused" : " is running"))
                    .font(.system(size: 13, weight: .semibold))
                Text(verbatim: "\(rows.count) agents" + (paused > 0 ? " · \(paused) paused" : ""))
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            Spacer()
            if let outcome {
                let shown = Presentation.present(outcome, hostName: hostName)
                StatePill(word: shown.title, symbol: "info.circle", tone: shown.tone)
            }
            if project.run != nil, decide?.decide == true {
                switch control.action {
                case .pause:
                    Button { askingProjectPause = true } label: { Text("Pause project…") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(!control.state.isEnabled)
                case .resume:
                    Button { sendRun(.runResume(reason: nil)) } label: { Text("Resume project") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(!control.state.isEnabled)
                }
            }
        }
        .padding(Tokens.space[2])
        .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
        .help(disabledReason(control.state)) // static-text
    }

    // MARK: Table

    private func table(_ rows: [AgentRow]) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: Tokens.space[2]) {
                head("Agent", width: 120)
                head("State", width: 70)
                head("Branch and task", width: nil)
                head("Health", width: 110)
            }
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            ForEach(rows, id: \.source.agentId) { row in
                Divider()
                agentRow(row)
            }
        }
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }

    private func head(_ title: String, width: CGFloat?) -> some View {
        Text(verbatim: title)
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Tokens.color(.label2))
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
    }

    private func agentRow(_ row: AgentRow) -> some View {
        let selected = model.selectedAgent == row.source.agentId
        return Button {
            model.selectedAgent = row.source.agentId
        } label: {
            HStack(spacing: Tokens.space[2]) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(verbatim: row.agentId).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    Text(verbatim: row.role).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                }
                .frame(width: 120, alignment: .leading)
                Text(verbatim: row.state).font(.system(size: 13)).frame(width: 70, alignment: .leading)
                VStack(alignment: .leading, spacing: 0) {
                    Text(verbatim: row.branch ?? "–").font(.system(size: 12, design: .monospaced)).lineLimit(1)
                    if let task = row.task {
                        Text(verbatim: task).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    } else if row.taskUnverified {
                        Text("The host could not vouch for the task, so it is not shown.")
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    } else {
                        Text("No task").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health))
                    .frame(width: 110, alignment: .leading)
            }
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            .background(selected ? Tokens.color(.accent).opacity(0.16) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func sendRun(_ verb: DecideVerb) {
        outcome = nil
        Task {
            outcome = await model.decideRun(verb, hostId: hostId, projectId: projectId)
        }
    }

    private func disabledReason(_ state: ControlState) -> String {
        if case .disabled(let reason) = state { return reason }
        return ""
    }
}

enum AgentTone {
    static func tone(_ health: AgentHealth) -> Tone {
        switch health {
        case .working: return .green
        case .needsYou: return .accent
        case .stalled: return .yellow
        case .paused: return .grey
        case .lost, .blocked: return .red
        case .idle: return .grey
        }
    }
}

/// The agent the Agents table selected: what it is and Pause and Resume with an optional reason.
struct AgentInspector: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String

    @State private var reason = ""
    @State private var outcome: DecideOutcome?
    @State private var sending = false

    private var budget: TextBudget { TextBudget(limit: ProtocolLimits.reasonCap) }

    var body: some View {
        if let project = model.project(hostId, projectId), let id = model.selectedAgent,
            let stored = project.agents[id] ?? project.agents.values.first(where: { $0.agentId == id })
        {
            content(AgentRow(agent: stored, in: project), stored, project)
                .onChange(of: id) { _, _ in
                    reason = ""
                    outcome = nil
                }
        } else {
            Text("Select an agent to see what it is doing")
                .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func content(_ row: AgentRow, _ stored: StoredAgent, _ project: ProjectModel) -> some View {
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: stored.requestId)
        let control = PauseControl(
            capabilities: model.capabilities[hostId], busy: model.busy.keys, key: key, paused: stored.paused)
        let overLimit = !budget.allows(reason)
        return VStack(alignment: .leading, spacing: Tokens.space[2]) {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    HStack(spacing: Tokens.space[1]) {
                        StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health))
                        Text(verbatim: HostLabel.projectName(project)).font(.system(size: 12))
                            .foregroundStyle(Tokens.color(.label2))
                    }
                    Text(verbatim: row.agentId + " is " + row.healthWord.lowercased())
                        .font(.system(size: 20, weight: .semibold))
                    MetaList(rows: [
                        MetaRow(label: "Role", value: plain(row.role + " · generation " + String(stored.generation))),
                        MetaRow(label: "Branch", value: plain(row.branch ?? "–")),
                        MetaRow(label: "State", value: plain(row.state + (stored.paused ? ", paused" : ", not paused"))),
                        MetaRow(label: "Request", value: plain(stored.requestId)),
                    ])
                    Text(verbatim: Self.explanation(row.health)).font(.system(size: 13))
                    if let basis = AgentHealth.basis(of: stored, in: project).line(now: Date()) {
                        Text(verbatim: basis).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                    if let task = row.task {
                        UntrustedTextBlock(text: plain(task), label: "Task · untrusted text")
                    }
                    if let outcome {
                        let shown = Presentation.present(outcome, hostName: hostName)
                        StatePill(word: shown.title, symbol: "info.circle", tone: shown.tone)
                    }
                }
            }
            if model.capabilities[hostId]?.decide == true {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text("Reason for the pause").font(.system(size: 12, weight: .semibold))
                TextField("Optional", text: $reason)
                    .textFieldStyle(.roundedBorder)
                Text(verbatim: "Shown to the PM. Default: remote client " + model.settings.clientLabel + ".")
                    .font(.system(size: 12))
                    .foregroundStyle(overLimit ? Tokens.color(.red) : Tokens.color(.label2))
                HStack {
                    Spacer()
                    Button { send(.agentResume(reason: NoteText.normalized(reason)), stored) } label: { Text("Resume") }
                        .buttonStyle(.capstanQuiet)
                        .disabled(sending || overLimit || !control.resume.isEnabled)
                    Button { send(.agentPause(reason: NoteText.normalized(reason)), stored) } label: {
                        Text(verbatim: "Pause " + row.agentId)
                    }
                    .buttonStyle(.capstanPrimary)
                    .disabled(sending || overLimit || !control.pause.isEnabled)
                }
            }
            } else {
                Text("This host's Capstan has no remote decisions. Pause and resume from the terminal.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(Tokens.space[3])
    }

    private var hostName: String {
        model.hosts.first { $0.id == hostId }.map(HostLabel.hostName) ?? "the host"
    }

    private func plain(_ s: String) -> UntrustedText {
        UntrustedText(raw: s, verified: true, truncated: false)
    }

    private func send(_ verb: DecideVerb, _ stored: StoredAgent) {
        sending = true
        outcome = nil
        Task {
            outcome = await model.decideAgent(verb, hostId: hostId, projectId: projectId, agentId: stored.agentId)
            sending = false
        }
    }

    static func explanation(_ health: AgentHealth) -> String {
        switch health {
        case .working:
            return "Working means the host sees the agent busy in its terminal (activity working). When the host gives no activity, or one this app does not know, Working is an estimate from state active and a task; it does not say the agent made progress."
        case .needsYou:
            return "Needs you means a prompt from this agent waits for your answer in Waiting for you, or the host sees the agent stopped at a question in its terminal (activity blocked). If Waiting for you has no prompt from it, answer in the agent's terminal."
        case .stalled:
            return "Stalled means the controller flagged the agent for attention: it saw no progress for longer than its limit. The connection is fine. You can pause the agent while you look, or leave it to the supervisor."
        case .lost:
            return "Lost means the controller marked the agent lost (state lost): its pane is gone and the controller will not hear from it again."
        case .blocked:
            return "Blocked means the agent told the controller it cannot go on without something from outside (state blocked)."
        case .paused: return "Paused means the agent was paused (paused, or state paused). Resume it when you are ready."
        case .idle:
            return "Idle means the host sees the agent waiting at its prompt or finished with its turn (activity idle or done). When the host gives no activity, or one this app does not know, Idle is an estimate: the agent is not active or has no task."
        }
    }
}

#Preview("Agents: every health") {
    let seed = PreviewData.standard
    AgentsScreen(model: AppModel(preview: seed), hostId: PreviewData.alphaId, projectId: "capstan")
        .frame(width: 560, height: 640)
}

#Preview("Agents: host without decisions") {
    AgentsScreen(model: AppModel(preview: PreviewData.standard), hostId: PreviewData.betaId, projectId: "api")
        .frame(width: 560, height: 300)
}
