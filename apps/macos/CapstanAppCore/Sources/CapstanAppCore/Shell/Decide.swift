import CapstanKit
import Foundation

/// What a decision is about: an inbox row, an agent or the run.
public enum RouteTarget: Sendable, Equatable {
    case row(key: String)
    case agent(id: String)
    case run
    /// The project's active PM: a free message (`pm message`).
    case pm
    /// A plan of the `plans` section, by its item key (`plan cancel`).
    case plan(key: String)
}

/// The request id and content hash a decision must carry, read from the model the person is looking at.
public enum RequestTarget {
    public static func forRow(_ p: ProjectModel, key: String) -> (requestId: String, expect: String)? {
        guard let item = p.waiting[key] else { return nil }
        return (item.requestId, item.contentHash)
    }

    public static func forAgent(_ p: ProjectModel, id: String) -> (requestId: String, expect: String)? {
        guard let item = p.agents["agent:" + id] else { return nil }
        return (item.requestId, item.contentHash)
    }

    /// The PM binding; nil without exactly one active PM.
    public static func forPm(_ p: ProjectModel) -> (requestId: String, expect: String)? {
        guard let agentId = PmBinding.activePm(p) else { return nil }
        return (PmBinding.requestId, PmBinding.hash(agentId: agentId))
    }

    /// The plan's request and binding; nil once it is gone, or from a host that does not send them.
    public static func forPlan(_ p: ProjectModel, key: String) -> (requestId: String, expect: String)? {
        guard let plan = p.plans[key], let requestId = plan.requestId, let hash = plan.contentHash else { return nil }
        return (requestId, hash)
    }

    public static func forRun(_ p: ProjectModel) -> (requestId: String, expect: String)? {
        guard let item = p.run, item.hashKnown else { return nil }
        return (item.requestId, item.contentHash)
    }
}

/// Outcomes that come from this client and never from a host.
public enum LocalOutcome {
    public static func notWaiting() -> DecideOutcome {
        .refused(
            DecideRefusal(
                code: .notWaiting,
                message: UntrustedText(raw: "This item is not waiting any more", verified: true, truncated: false)),
            hint: .gone)
    }

    /// No PM is active to send a message to: refused here, as the host would.
    public static func noActivePm() -> DecideOutcome {
        .refused(
            DecideRefusal(
                code: .noActivePm,
                message: UntrustedText(raw: "There is no active PM in this project", verified: true, truncated: false)),
            hint: .noPm)
    }

    /// Hosts send only the fixed codes of the protocol, so `local-busy` cannot come from a host.
    public static func busy() -> DecideOutcome {
        .refused(
            DecideRefusal(
                code: .other("local-busy"),
                message: UntrustedText(raw: "A decision for this item is already being sent", verified: true, truncated: false)),
            hint: .generic)
    }
}

public enum BusyGate {
    /// The local refusal when a decision for the key is already on its way.
    public static func check(_ busy: Set<BusyKey>, _ key: BusyKey) -> DecideOutcome? {
        busy.contains(key) ? LocalOutcome.busy() : nil
    }
}

public enum Route: Sendable, Equatable {
    case send(DecideRequest, BusyKey)
    case local(DecideOutcome)
}

public enum DecideRouter {
    public static func route(
        _ verb: DecideVerb, target: RouteTarget, project: ProjectModel?, busy: Set<BusyKey>, hostId: UUID,
        projectId: String, client: String, nonce: String
    ) -> Route {
        guard let project else { return .local(LocalOutcome.notWaiting()) }
        let found: (requestId: String, expect: String)?
        switch target {
        case .row(let key): found = RequestTarget.forRow(project, key: key)
        case .agent(let id): found = RequestTarget.forAgent(project, id: id)
        case .run: found = RequestTarget.forRun(project)
        case .pm: found = RequestTarget.forPm(project)
        case .plan(let key): found = RequestTarget.forPlan(project, key: key)
        }
        guard let found else {
            return .local(target == .pm ? LocalOutcome.noActivePm() : LocalOutcome.notWaiting())
        }
        let key = BusyKey(hostId: hostId, projectId: projectId, requestId: found.requestId)
        if let refusal = BusyGate.check(busy, key) { return .local(refusal) }
        let request = DecideRequest(
            hostId: hostId, projectId: projectId, requestId: found.requestId, expect: found.expect, nonce: nonce,
            client: client, verb: verb)
        return .send(request, key)
    }
}

public enum ForwardRecording {
    /// The record to keep for a forwarded decision; nothing for any other outcome.
    public static func record(_ o: DecideOutcome, at: Date) -> ForwardRecord? {
        guard case .forwarded(let result) = o else { return nil }
        return ForwardRecord(
            requestId: result.requestId, contentHash: result.contentHash, messageId: result.messageId, at: at)
    }
}

public enum NoteText {
    /// The text without surrounding white space; nil when nothing is left.
    public static func normalized(_ s: String) -> String? {
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

public struct BusyTracker: Sendable {
    public private(set) var keys: Set<BusyKey>

    public init(keys: Set<BusyKey> = []) {
        self.keys = keys
    }

    /// False when the key is already present.
    public mutating func begin(_ k: BusyKey) -> Bool {
        keys.insert(k).inserted
    }

    public mutating func end(_ k: BusyKey) {
        keys.remove(k)
    }
}

public enum ForwardPill {
    /// True while the item is the one that was forwarded: both exist and the content hash is the same.
    public static func shown(item: StoredWaiting?, record: ForwardRecord?) -> Bool {
        guard let item, let record else { return false }
        return item.contentHash == record.contentHash
    }
}

/// What the op body of the inspector offers for an Operator proposal.
public enum ApproveOffer: Sendable, Equatable {
    /// The host does not send `approveChoices`: the one button that sends the approval to the PM.
    case forward
    /// The host sends `approveChoices` and the command is shown whole and checked: the Answer… sheet.
    case sheet(ApproveFlow)
    /// The host sends `approveChoices` but nothing may be approved from here (the command is cut, does not match
    /// its hash, is not here yet, or is not checked); Deny stays.
    case denyOnly(ApproveLimit?)

    public static func make(_ item: StoredWaiting, command: UntrustedText?) -> ApproveOffer {
        guard let choices = item.approveChoices else { return .forward }
        let whole = command.map { $0.verified && !$0.truncated } ?? false
        if let flow = ApproveFlow(choices: choices, commandVerified: whole) { return .sheet(flow) }
        return .denyOnly(ApproveLimit.make(choices) ?? appLimit(choices, command: command))
    }

    /// Why the app itself holds the approval back when the host named no limit: the command it has is cut or
    /// not checked. nil while the command has not arrived (the inspector says so).
    private static func appLimit(_ choices: ApproveChoices, command: UntrustedText?) -> ApproveLimit? {
        guard let command else { return nil }
        let reason = command.verified ? ApproveStrings.limitedTruncated : ApproveStrings.limitedTextMismatch
        return ApproveLimit(lead: nil, reason: reason, floorEntry: nil)
    }
}
