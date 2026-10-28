import CapstanAppCore
import CapstanKit
import SwiftUI

/// The agents of a project with their health, and Pause and Resume for each agent and for the project.
struct AgentsScreen: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String

    @State private var outcomes: [String: DecideOutcome] = [:]
    @State private var askingProjectPause = false
    @State private var reasonFor: ReasonRequest?

    private static let runKey = "run"

    struct ReasonRequest: Identifiable {
        let agentId: String
        let resume: Bool
        var id: String { agentId + (resume ? "-resume" : "-pause") }
    }

    private var project: ProjectModel? { model.project(hostId, projectId) }
    private var decide: HostCapabilities? { model.capabilities[hostId] }
    private var hostName: String {
        model.hosts.first { $0.id == hostId }.map(HostLabel.hostName) ?? "the host"
    }

    var body: some View {
        if let project {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    projectBar(project)
                    let rows = AgentRow.rows(project)
                    if rows.isEmpty {
                        Text("No agents yet").foregroundStyle(Tokens.color(.label2))
                    }
                    ForEach(rows, id: \.source.agentId) { row in
                        agentRow(row, stored: row.source)
                    }
                }
                .padding(Tokens.space[4])
            }
            .sheet(isPresented: $askingProjectPause) {
                ConfirmSheet(
                    title: "Pause the project?", detail: nil, noteLimit: ProtocolLimits.reasonCap,
                    destructiveLabel: "Pause"
                ) { reason in
                    sendRun(.runPause(reason: reason))
                }
            }
            .sheet(item: $reasonFor) { request in
                ReasonSheet(resume: request.resume) { reason in
                    sendAgent(request.agentId, request.resume ? .agentResume(reason: reason) : .agentPause(reason: reason))
                }
            }
        } else {
            Text("This project is not available").foregroundStyle(Tokens.color(.label2)).padding()
        }
    }

    // MARK: Project

    private func projectBar(_ project: ProjectModel) -> some View {
        let runPaused = project.run?.paused ?? false
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: project.run?.requestId ?? Self.runKey)
        let control = ProjectPauseControl(
            capabilities: decide, busy: model.busy.keys, key: key, runPaused: runPaused)
        return HStack(spacing: Tokens.space[1]) {
            if let status = control.status {
                StatePill(word: status, symbol: AgentHealth.paused.symbolName, tone: .yellow)
            }
            Spacer()
            outcomePill(for: Self.runKey, busy: model.busy.keys.contains(key))
            if project.run != nil {
                switch control.action {
                case .pause:
                    Button("Pause project") { askingProjectPause = true }
                        .disabled(!control.state.isEnabled)
                case .resume:
                    Button("Resume project") { sendRun(.runResume(reason: nil)) }
                        .disabled(!control.state.isEnabled)
                }
            }
        }
        .help(disabledReason(control.state)) // static-text
    }

    // MARK: Agents

    private func agentRow(_ row: AgentRow, stored: StoredAgent) -> some View {
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: stored.requestId)
        let control = PauseControl(capabilities: decide, busy: model.busy.keys, key: key, paused: stored.paused)
        let busy = model.busy.keys.contains(key)
        return VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: row.role).font(.system(size: 13, weight: .semibold))
                Text(verbatim: row.state).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                Spacer()
                outcomePill(for: stored.agentId, busy: busy)
                StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: tone(row.health))
            }
            if let task = row.task {
                Text(verbatim: task).font(.system(size: 12)).lineLimit(1).truncationMode(.tail)
            } else if row.taskUnverified {
                Text("The host could not vouch for the task, so it is not shown.")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
            }
            HStack(spacing: Tokens.space[1]) {
                Button("Pause") { reasonFor = ReasonRequest(agentId: stored.agentId, resume: false) }
                    .disabled(!control.pause.isEnabled)
                    .help(disabledReason(control.pause)) // static-text
                Button("Resume") { reasonFor = ReasonRequest(agentId: stored.agentId, resume: true) }
                    .disabled(!control.resume.isEnabled)
                    .help(disabledReason(control.resume)) // static-text
            }
            .controlSize(.small)
        }
        .padding(Tokens.space[2])
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
    }

    // MARK: Sending

    private func sendAgent(_ agentId: String, _ verb: DecideVerb) {
        outcomes[agentId] = nil
        Task {
            let outcome = await model.decideAgent(verb, hostId: hostId, projectId: projectId, agentId: agentId)
            // The health changes only from the stream update; this is the answer to the request.
            outcomes[agentId] = outcome
        }
    }

    private func sendRun(_ verb: DecideVerb) {
        outcomes[Self.runKey] = nil
        Task {
            outcomes[Self.runKey] = await model.decideRun(verb, hostId: hostId, projectId: projectId)
        }
    }

    @ViewBuilder
    private func outcomePill(for id: String, busy: Bool) -> some View {
        if busy {
            let sending = Presentation.present(nil, hostName: hostName)
            StatePill(word: sending.title, symbol: "paperplane.fill", tone: sending.tone)
        } else if let outcome = outcomes[id] {
            let shown = Presentation.present(outcome, hostName: hostName)
            StatePill(word: shown.title, symbol: symbol(shown.tone), tone: shown.tone)
        }
    }

    private func symbol(_ tone: Tone) -> String {
        switch tone {
        case .green: return "checkmark.circle.fill"
        case .red: return "xmark.octagon.fill"
        case .yellow: return "exclamationmark.triangle.fill"
        case .blue: return "arrow.right.circle.fill"
        case .neutral, .grey: return "info.circle.fill"
        }
    }

    private func tone(_ health: AgentHealth) -> Tone {
        switch health {
        case .working: return .green
        case .needsYou: return .blue
        case .stalled, .paused: return .yellow
        case .lost, .blocked: return .red
        case .idle: return .grey
        }
    }

    private func disabledReason(_ state: ControlState) -> String {
        if case .disabled(let reason) = state { return reason }
        return ""
    }
}

/// An optional reason for pausing or resuming one agent, counted in UTF-8 bytes.
private struct ReasonSheet: View {
    let resume: Bool
    let onSend: (String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var reason = ""

    private var budget: TextBudget { TextBudget(limit: ProtocolLimits.reasonCap) }
    private var overLimit: Bool { !budget.allows(reason) }

    private func submit() {
        onSend(NoteText.normalized(reason))
        dismiss()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: resume ? "Resume this agent?" : "Pause this agent?")
                .font(.system(size: 15, weight: .semibold))
            TextField("Reason (optional)", text: $reason, axis: .vertical)
                .lineLimit(2...5)
                .textFieldStyle(.roundedBorder)
            Text(verbatim: String(budget.used(reason)) + " of " + String(budget.limit) + " bytes")
                .font(.system(size: 12).monospacedDigit())
                .foregroundStyle(overLimit ? Tokens.color(.red) : Tokens.color(.label2))
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                if resume {
                    Button("Resume", action: submit).keyboardShortcut(.defaultAction).disabled(overLimit)
                } else {
                    Button("Pause", action: submit).keyboardShortcut(.defaultAction).disabled(overLimit)
                }
            }
        }
        .padding(Tokens.space[4])
        .frame(width: 420)
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
