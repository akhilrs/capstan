import Foundation

/// A waiting item that is new to the person. It holds no text: the app reads the text from the model when it
/// builds the notification.
public struct NotifyCandidate: Sendable, Equatable {
    public let hostId: UUID
    public let projectId: String
    public let kind: WaitingKind
    public let key: String

    public init(hostId: UUID, projectId: String, kind: WaitingKind, key: String) {
        self.hostId = hostId
        self.projectId = projectId
        self.kind = kind
        self.key = key
    }
}

public struct ReduceOutput: Sendable, Equatable {
    /// Projects whose snapshot has to be read again.
    public var refresh: Set<String>
    /// The projects of `refresh` whose read is wanted now: the run changed or a decision was made, and the person
    /// waits for the item to follow. The session still limits how often it restarts for this.
    public var urgent: Set<String>
    public var notify: [NotifyCandidate]

    public init(refresh: Set<String> = [], urgent: Set<String> = [], notify: [NotifyCandidate] = []) {
        self.refresh = refresh
        self.urgent = urgent
        self.notify = notify
    }
}

/// What the reducer keeps beside the host model: snapshot lines that wait for their `snapshotEnd`, and the waiting
/// keys of the last complete snapshot of each project, which decide what is new.
public struct Staging: Sendable, Equatable {
    struct Pending: Sendable, Equatable {
        var waiting: [String: StoredWaiting] = [:]
        var agents: [String: StoredAgent] = [:]
        var plans: [String: StoredPlan] = [:]
        var run: StoredRun?
    }

    var pending: [String: Pending] = [:]
    /// Absent for a project that has no complete snapshot yet, in memory or loaded.
    var knownKeys: [String: Set<String>] = [:]

    public init() {}

    /// The projects with snapshot lines still waiting for their end.
    public var stagedProjectIds: Set<String> {
        Set(pending.keys)
    }

    /// Seeds the notify baseline of a project from keys that were complete before this run.
    public mutating func seed(projectId: String, waitingKeys: Set<String>) {
        knownKeys[projectId] = waitingKeys
    }
}

public enum StoreReducer {
    private static let sections: Set<String> = ["waiting", "agents", "plans", "run"]

    /// Applies one line. `now` stamps a project that stops being ready.
    public static func apply(
        _ line: StreamLine, to host: inout HostModel, staging: inout Staging, now: Date = Date()
    ) -> ReduceOutput {
        var output = ReduceOutput()
        switch line {
        case .hello(let hello):
            host.hello = HostHello(
                version: plain(hello.version), host: plain(hello.host), projects: hello.projects)
            // A new stream starts every snapshot again.
            staging.pending = [:]
        case .project(let project):
            applyProject(project, to: &host, now: now)
        case .snapshot(let snapshot):
            // The first line of a snapshot starts the refresh; the last-known values stay until its end.
            if staging.pending[snapshot.projectId] == nil {
                host.projects[snapshot.projectId]?.markFreshness { $0.refreshing = true }
            }
            stage(snapshot, into: &staging)
        case .snapshotEnd(let projectId, let head):
            applySnapshotEnd(
                projectId: projectId, head: head, host: &host, staging: &staging, output: &output, now: now)
        case .event(let projectId, let event):
            applyEvent(event, projectId: projectId, host: &host, staging: &staging, output: &output, now: now)
        case .cursor(let cursor):
            host.cursor = cursor
        case .resync(let projectId, _):
            if let projectId {
                drop(projectId, from: &host, staging: &staging)
            } else {
                for id in Array(host.projects.keys) { drop(id, from: &host, staging: &staging) }
                staging.pending = [:]
            }
        case .heartbeat, .error, .unknownKind:
            break
        }
        return output
    }

    private static func plain(_ text: String) -> UntrustedText {
        UntrustedText(raw: text, verified: true, truncated: false)
    }

    // MARK: Lines

    private static func applyProject(_ line: ProjectLine, to host: inout HostModel, now: Date) {
        var model = host.projects[line.projectId] ?? ProjectModel(projectId: line.projectId, root: line.root)
        model.root = line.root
        model.availability = line.state
        if line.state == .ready {
            model.epoch = line.epoch
            model.head = line.head
            model.staleSince = nil
            // A ready project without a complete snapshot is about to receive one, an empty one included.
            if !model.snapshotComplete { model.markFreshness { $0.refreshing = true } }
        } else if model.staleSince == nil {
            model.staleSince = now
        }
        host.projects[line.projectId] = model
    }

    private static func stage(_ line: SnapshotLine, into staging: inout Staging) {
        var pending = staging.pending[line.projectId] ?? Staging.Pending()
        switch line.item {
        case .waiting(let item): pending.waiting[line.key] = StoredWaiting(item)
        case .agent(let item): pending.agents[line.key] = StoredAgent(item)
        case .plan(let item): pending.plans[line.key] = StoredPlan(item)
        case .run(let item): pending.run = StoredRun(item)
        case .unknown: break
        }
        staging.pending[line.projectId] = pending
    }

    private static func applySnapshotEnd(
        projectId: String, head: Int64, host: inout HostModel, staging: inout Staging, output: inout ReduceOutput,
        now: Date
    ) {
        let pending = staging.pending.removeValue(forKey: projectId) ?? Staging.Pending()
        guard var model = host.projects[projectId] else { return }
        let keys = Set(pending.waiting.keys)
        if let known = staging.knownKeys[projectId] {
            for key in keys.subtracting(known).sorted() {
                guard let item = pending.waiting[key] else { continue }
                output.notify.append(
                    NotifyCandidate(hostId: host.hostId, projectId: projectId, kind: item.kind, key: key))
            }
        }
        staging.knownKeys[projectId] = keys
        model.waiting = pending.waiting
        model.agents = pending.agents
        model.plans = pending.plans
        model.run = pending.run
        // A detail may come from an event newer than the snapshot, for an item it does not hold yet.
        model.details = model.details.filter { keys.contains($0.key) || $0.value.seq > head }
        model.attention = [:]
        model.head = head
        model.snapshotComplete = true
        model.markFreshness { $0.complete(at: now) }
        host.projects[projectId] = model
    }

    /// Forgets what is held of a project; the notify baseline stays.
    private static func drop(_ projectId: String, from host: inout HostModel, staging: inout Staging) {
        staging.pending[projectId] = nil
        guard var model = host.projects[projectId] else { return }
        model.waiting = [:]
        model.agents = [:]
        model.plans = [:]
        model.run = nil
        model.details = [:]
        model.attention = [:]
        model.snapshotComplete = false
        model.markFreshness { $0.refreshing = true }
        host.projects[projectId] = model
    }

    /// The stream ended or failed: a project that waited for a read keeps what it knew and notes the failure.
    public static func noteRefreshFailure(in host: inout HostModel, now: Date) {
        for id in Array(host.projects.keys) {
            guard let marks = host.projects[id]?.freshness, marks.isOutstanding else { continue }
            host.projects[id]?.markFreshness { $0.failRefresh(at: now) }
        }
    }

    // MARK: Events

    private static func applyEvent(
        _ event: FeedEvent, projectId: String, host: inout HostModel, staging: inout Staging,
        output: inout ReduceOutput, now: Date
    ) {
        guard var model = host.projects[projectId] else { return }
        if let head = model.head {
            if event.seq > head { model.head = event.seq }
        } else {
            model.head = event.seq
        }
        applyAttention(event, to: &model)

        var unknown = false
        for effect in event.body.effects {
            guard sections.contains(effect.section) else {
                unknown = true
                break
            }
            switch effect.op {
            case .other:
                unknown = true
            case .invalidate:
                let pendingKey = FreshnessMarks.pendingKey(section: effect.section, key: effect.key)
                model.markFreshness { $0.pending.insert(pendingKey) }
                output.refresh.insert(projectId)
                if effect.section == "run" || event.type == "decision.remote" { output.urgent.insert(projectId) }
            case .remove:
                remove(effect, from: &model)
            case .upsert:
                if !upsert(effect, in: &model, hostId: host.hostId, staging: &staging, output: &output) {
                    let pendingKey = FreshnessMarks.pendingKey(section: effect.section, key: effect.key)
                    model.markFreshness { $0.pending.insert(pendingKey) }
                    output.refresh.insert(projectId)
                }
            }
            if unknown { break }
            noteDetail(effect, of: event, in: &model)
        }
        if !unknown {
            applyRunChange(event, to: &model)
            model.markFreshness { $0.noteEvent(at: now) }
        }
        host.projects[projectId] = model
        if unknown {
            drop(projectId, from: &host, staging: &staging)
            output.refresh.insert(projectId)
        }
    }

    /// `run.changed` states the new run state and pause, but its only effect is an invalidate of the run item. The
    /// stored run follows the event now. Its content hash also covers the open pause id, which the event does not
    /// state, so the old hash is dropped (empty) and the re-read that the invalidate asks for brings the new one.
    private static func applyRunChange(_ event: FeedEvent, to model: inout ProjectModel) {
        guard let body = event.body.runBody, let run = model.run else { return }
        model.run = StoredRun(
            requestId: run.requestId, state: body.state, paused: body.paused, contentHash: "")
    }

    private static func applyAttention(_ event: FeedEvent, to model: inout ProjectModel) {
        if event.type == "plan.needs_attention", event.entity.type == "agent" {
            if let kind = try? event.body.raw.string("state") { model.attention[event.entity.id] = kind }
        } else if event.type.hasPrefix("agent."), event.type != "agent.activity" {
            // An activity change is the host's observation, not news from the agent, so it leaves a stall flag alone.
            let agentId = event.entity.type == "agent" ? event.entity.id : (try? event.body.raw.string("agentId"))
            if let agentId { model.attention[agentId] = nil }
        }
    }

    private static func remove(_ effect: Effect, from model: inout ProjectModel) {
        switch effect.section {
        case "waiting":
            model.waiting[effect.key] = nil
            model.details[effect.key] = nil
        case "agents":
            model.agents[effect.key] = nil
            model.attention[effect.key] = nil
        case "plans":
            model.plans[effect.key] = nil
        default:
            model.run = nil
        }
    }

    /// Merges the fields into the item, or creates it. False when the result is not an item.
    private static func upsert(
        _ effect: Effect, in model: inout ProjectModel, hostId: UUID, staging: inout Staging,
        output: inout ReduceOutput
    ) -> Bool {
        let key = effect.key
        switch effect.section {
        case "waiting":
            let json = merged(model.waiting[key]?.json, effect)
            guard let item = try? WaitingItem(json: json) else { return false }
            var stored = StoredWaiting(item)
            // Text the effect does not state keeps the flags it was stored with; they are not read back from raw text.
            if let base = model.waiting[key], effect.fields.optional("summary") == nil { stored.summary = base.summary }
            model.waiting[key] = stored
            // A key notified here, during a snapshot, is not notified again by that snapshot's end: the key joins the
            // baseline now, so `applySnapshotEnd` finds it known.
            if var known = staging.knownKeys[model.projectId], !known.contains(key) {
                output.notify.append(
                    NotifyCandidate(hostId: hostId, projectId: model.projectId, kind: stored.kind, key: key))
                known.insert(key)
                staging.knownKeys[model.projectId] = known
            }
        case "agents":
            let incomingSeq = effect.fields.activitySeq()
            // Activity only adds to an agent the client holds; for any other key there is nothing to do, not a refresh.
            if incomingSeq != nil, model.agents[key] == nil { return true }
            guard let item = try? AgentItem(json: merged(model.agents[key]?.json, effect)) else { return false }
            var stored = StoredAgent(item)
            if let base = model.agents[key], effect.fields.optional("task") == nil { stored.task = base.task }
            if let base = model.agents[key], effect.fields.optional("branch") == nil { stored.branch = base.branch }
            // The three activity fields move together, and only to a greater sequence than the one held.
            if let base = model.agents[key] {
                let newer = incomingSeq.map { $0 > (base.activitySeq ?? 0) } ?? false
                if !newer {
                    stored.activity = base.activity
                    stored.activitySince = base.activitySince
                    stored.activitySeq = base.activitySeq
                }
            }
            model.agents[key] = stored
        case "plans":
            guard let item = try? PlanItem(json: merged(model.plans[key]?.json, effect)) else { return false }
            var stored = StoredPlan(item)
            if let base = model.plans[key], effect.fields.optional("title") == nil { stored.title = base.title }
            model.plans[key] = stored
        default:
            guard let item = try? RunItem(json: merged(model.run?.json, effect)) else { return false }
            model.run = StoredRun(item)
        }
        return true
    }

    /// The item as JSON with the fields laid over it. What the effect itself states (the key) fills a missing member.
    private static func merged(_ base: JSONValue?, _ effect: Effect) -> JSONValue {
        let combined = (base ?? .object([:])).merging(fields: effect.fields)
        guard case .object(var members) = combined else { return combined }
        func fill(_ name: String, _ value: JSONValue?) {
            if combined.optional(name) == nil, let value { members[name] = value }
        }
        let key = effect.key
        switch effect.section {
        case "waiting":
            fill("key", .string(key))
            fill("requestId", .string(key))
            fill("kind", .string(String(key.split(separator: ":", maxSplits: 1).first ?? "")))
        case "agents":
            fill("key", .string(key))
            fill("agentId", .string(key))
        case "plans":
            fill("key", .string(key))
            fill("sequence", Int64(key).map { JSONValue.int($0) })
        default:
            fill("requestId", .string("run"))
        }
        return .object(members)
    }

    /// Keeps the event that named a waiting key, unless it only says that the item changed.
    private static func noteDetail(_ effect: Effect, of event: FeedEvent, in model: inout ProjectModel) {
        guard effect.section == "waiting", effect.op != .remove, effect.key.contains(":"),
              event.type != "snapshot.invalidated" else { return }
        model.details[effect.key] = ItemDetail(
            key: effect.key, eventType: event.type, seq: event.seq, body: event.body.raw)
    }
}
