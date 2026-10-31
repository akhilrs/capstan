import CapstanAppCore
import CapstanKit
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
