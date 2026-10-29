import CapstanKit
import Foundation

/// What a decision is about: an inbox row, an agent or the run.
public enum RouteTarget: Sendable, Equatable {
    case row(key: String)
    case agent(id: String)
    case run
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

    public static func forRun(_ p: ProjectModel) -> (requestId: String, expect: String)? {
        guard let item = p.run else { return nil }
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
        }
        guard let found else { return .local(LocalOutcome.notWaiting()) }
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
