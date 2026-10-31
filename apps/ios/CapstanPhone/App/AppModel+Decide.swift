import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation

/// Decisions, as on the Mac: routed by `DecideRouter`, one at a time per request, sent through the shared runner and
/// kept in `PendingStore` when no answer came. A tap on the phone is never an approval: approvals and prompt answers
/// are forwarded to the PM.
extension AppModel {
    func decide(_ verb: DecideVerb, on row: InboxRowID) async -> DecideOutcome {
        await perform(verb, target: .row(key: row.key), hostId: row.hostId, projectId: row.projectId)
    }

    func decideAgent(_ verb: DecideVerb, hostId: UUID, projectId: String, agentId: String) async -> DecideOutcome {
        await perform(verb, target: .agent(id: agentId), hostId: hostId, projectId: projectId)
    }

    func decideRun(_ verb: DecideVerb, hostId: UUID, projectId: String) async -> DecideOutcome {
        await perform(verb, target: .run, hostId: hostId, projectId: projectId)
    }

    /// Sends a free message to the project's active PM and keeps it in the thread with its state (board 11).
    func messagePM(_ text: String, hostId: UUID, projectId: String) async -> DecideOutcome {
        let sent = SentMessage(hostId: hostId, projectId: projectId, text: text, sentAt: Date())
        messages.append(sent)
        let outcome = await perform(.pmMessage(text), target: .pm, hostId: hostId, projectId: projectId)
        settle(sent.id, outcome)
        return outcome
    }

    /// Sends a message that got no answer again, with its own nonce.
    func retryMessage(_ message: SentMessage) async -> DecideOutcome {
        guard case .notSent(_, let pending?) = message.state else {
            messages.update(message.id, state: .sending)
            let outcome = await perform(
                .pmMessage(message.text), target: .pm, hostId: message.hostId, projectId: message.projectId)
            settle(message.id, outcome)
            return outcome
        }
        messages.update(message.id, state: .sending)
        let outcome = await retry(pending)
        settle(message.id, outcome)
        return outcome
    }

    /// Answers a question an agent sent the person (`message reply`); the answer is kept in the thread, marked as the
    /// answer to that question, so a send again without a pending request still reads as one.
    func answerQuestion(_ text: String, question: StoredMessage, hostId: UUID, projectId: String) async -> DecideOutcome {
        let sent = SentMessage(
            hostId: hostId, projectId: projectId, text: "Answer to question " + String(question.sequence) + ": " + text,
            sentAt: Date())
        messages.append(sent)
        let row = InboxRowID(hostId: hostId, projectId: projectId, key: "message:" + question.key)
        let outcome = await decide(.messageReply(text), on: row)
        settle(sent.id, outcome)
        return outcome
    }

    private func settle(_ id: UUID, _ outcome: DecideOutcome) {
        switch outcome {
        case .applied(let result), .forwarded(let result):
            messages.update(id, state: .sent(messageId: result.messageId ?? result.decisionId))
        case .refused(let refusal, let hint):
            let reason: String
            switch hint {
            case .noPm: reason = "No active PM"
            case .limit: reason = "Too many messages this hour"
            case .refresh: reason = "The PM changed; read the project again"
            default:
                if case .other(let code) = refusal.code { reason = "Refused: " + code } else { reason = "Refused" }
            }
            messages.update(id, state: .notSent(reason: reason, pendingId: nil))
        case .notSent(_, let pending):
            let hostId = messages.log.messages.first(where: { $0.id == id })?.hostId
            let host = hosts.first(where: { $0.id == hostId })?.displayName ?? "The host"
            messages.update(id, state: .notSent(reason: host + " unreachable", pendingId: pending))
        case .malformed(_, let pending):
            messages.update(id, state: .notSent(reason: "Unexpected answer", pendingId: pending))
        }
    }

    private func perform(_ verb: DecideVerb, target: RouteTarget, hostId: UUID, projectId: String) async -> DecideOutcome {
        let route = DecideRouter.route(
            verb, target: target, project: project(hostId, projectId), busy: busy.keys, hostId: hostId,
            projectId: projectId, client: settings.clientLabel, nonce: Nonce.make())
        switch route {
        case .local(let outcome):
            return outcome
        case .send(let request, let key):
            guard let config = hosts.first(where: { $0.id == hostId }), let decider else {
                return LocalOutcome.notWaiting()
            }
            guard busy.begin(key) else { return LocalOutcome.busy() }
            defer { busy.end(key) }
            let outcome = await decider.send(request, config: config)
            await recordForward(outcome, key: key)
            return outcome
        }
    }

    /// Sends a kept request again with its own nonce.
    func retry(_ pendingId: UUID) async -> DecideOutcome {
        guard let pendingStore, let decider, let record = await pendingStore.record(pendingId),
            let config = hosts.first(where: { $0.id == record.request.hostId })
        else {
            return .malformed("There is no such pending request", pendingId: nil)
        }
        let key = BusyKey(
            hostId: record.request.hostId, projectId: record.request.projectId, requestId: record.request.requestId)
        guard busy.begin(key) else { return LocalOutcome.busy() }
        defer { busy.end(key) }
        let outcome = await decider.retry(pendingId, config: config)
        await recordForward(outcome, key: key)
        return outcome
    }

    private func recordForward(_ outcome: DecideOutcome, key: BusyKey) async {
        guard let record = ForwardRecording.record(outcome, at: Date()) else { return }
        forwards[key] = record
        await engines[key.hostId]?.recordForward(projectId: key.projectId, record)
    }
}
