import CapstanAppCore
import CapstanKit
import Foundation

/// Messages to the PM, merges and plan cancels, as on the iPhone: each goes through the same router and runner as the
/// other decisions, and the result is a `DecideOutcome`.
extension AppModel {
    /// Sends a free message to the project's active PM and keeps it in the thread with its state.
    func messagePM(_ text: String, hostId: UUID, projectId: String) async -> DecideOutcome {
        let message = SentMessage(hostId: hostId, projectId: projectId, text: text, sentAt: Date())
        keepSent(message)
        let outcome = await perform(.pmMessage(text), target: .pm, hostId: hostId, projectId: projectId)
        settle(message.id, outcome)
        return outcome
    }

    /// Sends a message that got no answer again, with its own nonce.
    func retryMessage(_ message: SentMessage) async -> DecideOutcome {
        setSent(message.id, .sending)
        let outcome: DecideOutcome
        if case .notSent(_, let pending?) = message.state {
            outcome = await retry(pending)
        } else {
            outcome = await perform(
                .pmMessage(message.text), target: .pm, hostId: message.hostId, projectId: message.projectId)
        }
        settle(message.id, outcome)
        return outcome
    }

    /// Answers a question an agent sent the person (`message reply`); the answer is kept in the thread, marked as the
    /// answer to that question.
    func answerQuestion(_ text: String, question: StoredMessage, hostId: UUID, projectId: String) async -> DecideOutcome {
        let message = SentMessage(
            hostId: hostId, projectId: projectId, text: "Answer to question " + String(question.sequence) + ": " + text,
            sentAt: Date())
        keepSent(message)
        let row = InboxRowID(hostId: hostId, projectId: projectId, key: "message:" + question.key)
        let outcome = await decide(.messageReply(text), on: row)
        settle(message.id, outcome)
        return outcome
    }

    /// The branches a merge of the project may go into (`cstan merge-targets`); `unsupported` without a runner (a
    /// preview) or from a host whose cstan predates the command.
    func mergeTargets(hostId: UUID, projectId: String) async -> MergeTargetsResult {
        guard decider != nil, let config = hosts.first(where: { $0.id == hostId }) else { return .unsupported }
        return await MergeTargetsCall(projectId: projectId).run(
            runner: LiveProcessRunner(), config: config, clock: SystemClock())
    }

    /// Merges an integration, into `branch` when one is given and into the project's own branch otherwise.
    func merge(_ row: InboxRowID, into branch: String?) async -> DecideOutcome {
        await decide(branch.map { .integrationMergeInto($0) } ?? .integrationMerge, on: row)
    }

    func discardIntegration(_ row: InboxRowID) async -> DecideOutcome {
        await decide(.integrationDiscard, on: row)
    }

    /// Cancels a plan of the `plans` section (`plan cancel`), bound to the plan as it is listed.
    func cancelPlan(key: String, hostId: UUID, projectId: String) async -> DecideOutcome {
        await perform(.planCancel, target: .plan(key: key), hostId: hostId, projectId: projectId)
    }

    // MARK: Sent messages

    private func keepSent(_ message: SentMessage) {
        updateSent {
            $0.append(message)
            return true
        }
    }

    private func setSent(_ id: UUID, _ state: SentMessage.State) {
        updateSent {
            $0.update(id, state: state)
            return true
        }
    }

    private func settle(_ id: UUID, _ outcome: DecideOutcome) {
        switch outcome {
        case .applied(let result), .forwarded(let result):
            setSent(id, .sent(messageId: result.messageId ?? result.decisionId))
        case .refused(let refusal, let hint):
            let reason: String
            switch hint {
            case .noPm: reason = "No active PM"
            case .limit: reason = "Too many messages this hour"
            case .refresh: reason = "The PM changed; read the project again"
            default:
                if case .other(let code) = refusal.code { reason = "Refused: " + code } else { reason = "Refused" }
            }
            setSent(id, .notSent(reason: reason, pendingId: nil))
        case .notSent(_, let pending):
            let hostId = sent.messages.first(where: { $0.id == id })?.hostId
            let host = hosts.first(where: { $0.id == hostId })?.displayName ?? "The host"
            setSent(id, .notSent(reason: host + " unreachable", pendingId: pending))
        case .malformed(_, let pending):
            setSent(id, .notSent(reason: "Unexpected answer", pendingId: pending))
        }
    }
}
