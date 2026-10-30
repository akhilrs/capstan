import Foundation

/// Holds the model of one host: reads what a `HostSession` delivers, keeps the model and saves it for the next run.
public enum ProjectRemoval: Sendable, Equatable {
    case removed
    /// The project is live (ready) or in a state that is not "gone" or "silent".
    case refusedLive
    case notFound
}

public actor HostStoreEngine: StreamConsumer {
    /// The shortest time between two saves caused by cursor lines.
    static let saveIntervalSeconds: TimeInterval = 2
    static let maxForwardsPerProject = 256

    private let hostId: UUID
    private let persistence: any PersistenceStore
    private let clock: any SessionClock
    private let onChange: @Sendable (HostModel) -> Void
    private let onNotify: @Sendable ([NotifyCandidate]) -> Void
    private let onRefresh: @Sendable (Set<String>) -> Void
    private let onUrgentRefresh: (@Sendable (Set<String>) -> Void)?
    private let onConnection: (@Sendable (ConnectionState) -> Void)?

    /// Projects the person removed: their entries stay out of the cursor until the host names them again.
    private var removedProjects: Set<String> = []
    private var model: HostModel
    private var staging = Staging()
    private var connectionState: ConnectionState = .connecting
    private var lastSaveAt: Date?
    /// A save was asked for within the interval; the host to write is built when it is written, not before.
    private var saveDirty = false
    private var saveTask: Task<Void, Never>?
    public private(set) var lastSaveError: String?
    /// How many times the host to save was built; a deferred save builds it once, when it writes.
    private(set) var persistedBuildCount = 0

    /// Loads what an earlier run saved before any session starts, so the models, the notify baseline and the resume
    /// cursor are in place when the first line arrives.
    public init(
        hostId: UUID, persistence: any PersistenceStore, clock: any SessionClock,
        onChange: @escaping @Sendable (HostModel) -> Void,
        onNotify: @escaping @Sendable ([NotifyCandidate]) -> Void,
        onRefresh: @escaping @Sendable (Set<String>) -> Void,
        onUrgentRefresh: (@Sendable (Set<String>) -> Void)? = nil,
        onConnection: (@Sendable (ConnectionState) -> Void)? = nil
    ) {
        self.hostId = hostId
        self.persistence = persistence
        self.clock = clock
        self.onChange = onChange
        self.onNotify = onNotify
        self.onRefresh = onRefresh
        self.onUrgentRefresh = onUrgentRefresh
        self.onConnection = onConnection
        var model = HostModel(hostId: hostId)
        var staging = Staging()
        if let saved = persistence.load(hostId: hostId) {
            model.projects = saved.projects
            model.cursor = saved.cursor
            model.forwards = saved.forwards
            for (id, project) in saved.projects where project.snapshotComplete {
                staging.seed(projectId: id, waitingKeys: Set(project.waiting.keys))
            }
        }
        self.model = model
        self.staging = staging
    }

    // MARK: Reading

    public func current() -> HostModel {
        model
    }

    public func currentConnection() -> ConnectionState {
        connectionState
    }

    /// Every recorded forward of the host.
    public func forwards() -> [ForwardRecord] {
        model.forwards.keys.sorted().flatMap { model.forwards[$0] ?? [] }
    }

    public func forwardsByProject() -> [String: [ForwardRecord]] {
        model.forwards
    }

    public func forwards(projectId: String) -> [ForwardRecord] {
        model.forwards[projectId] ?? []
    }

    // MARK: StreamConsumer

    public func apply(host: UUID, lines: [StreamLine]) async {
        guard host == hostId, !lines.isEmpty else { return }
        let now = clock.now()
        var refresh = Set<String>()
        var urgent = Set<String>()
        var notify: [NotifyCandidate] = []
        var cursorSeen = false
        for line in lines {
            if case .project(let project) = line { removedProjects.remove(project.projectId) }
            let output = StoreReducer.apply(line, to: &model, staging: &staging, now: now)
            refresh.formUnion(output.refresh)
            urgent.formUnion(output.urgent)
            notify.append(contentsOf: output.notify)
            if case .cursor = line { cursorSeen = true }
        }
        if !removedProjects.isEmpty, let cursor = model.cursor {
            model.cursor = (try? cursor.removing(removedProjects)) ?? nil
        }
        onChange(model)
        if !notify.isEmpty { onNotify(notify) }
        // Interim (plan-73 replaces the restart): a project that is not ready has nothing to read again.
        // A project whose snapshot is still arriving, or was never complete, is read in full anyway.
        refresh = refresh.filter {
            model.projects[$0]?.availability == .ready && model.projects[$0]?.snapshotComplete == true
                && staging.pending[$0] == nil
        }
        urgent.formIntersection(refresh)
        if let onUrgentRefresh, !urgent.isEmpty {
            onUrgentRefresh(urgent)
            refresh.subtract(urgent)
        }
        if !refresh.isEmpty { onRefresh(refresh) }
        if cursorSeen { scheduleSave() }
    }

    public func connection(host: UUID, _ state: ConnectionState) async {
        guard host == hostId else { return }
        connectionState = state
        onConnection?(state)
    }

    /// The cursor of the projects that hold a complete snapshot; the others are read again from their start.
    public func resumeCursor(host: UUID) async -> Cursor? {
        guard host == hostId else { return nil }
        return PersistedHost.trimmed(model.cursor, keeping: completeProjectIds())
    }

    /// The host does not accept the cursor: forget it and the saved file. The notify baseline stays.
    public func dropCursor(host: UUID) async {
        guard host == hostId else { return }
        model.cursor = nil
        cancelDeferredSave()
        lastSaveAt = nil
        do {
            try persistence.delete(hostId: hostId)
        } catch {
            lastSaveError = "\(error)"
        }
        onChange(model)
    }

    // MARK: App calls

    /// Records a decision that the host forwarded; the app calls it with the `requestId` and `contentHash` of the answer.
    public func recordForward(projectId: String, _ record: ForwardRecord) {
        var records = model.forwards[projectId] ?? []
        records.removeAll { $0.requestId == record.requestId }
        records.append(record)
        if records.count > HostStoreEngine.maxForwardsPerProject {
            records.removeFirst(records.count - HostStoreEngine.maxForwardsPerProject)
        }
        model.forwards[projectId] = records
        onChange(model)
        scheduleSave()
    }

    /// Drops a project the host cannot reach from the list, the saved file and the cursor. A live project is refused.
    /// The project comes back when the host names it again (a `project` line).
    @discardableResult
    public func removeProject(_ projectId: String) -> ProjectRemoval {
        guard let project = model.projects[projectId] else { return .notFound }
        guard project.availability.isRemovable else { return .refusedLive }
        model.projects[projectId] = nil
        model.forwards[projectId] = nil
        staging.pending[projectId] = nil
        staging.knownKeys[projectId] = nil
        removedProjects.insert(projectId)
        if let cursor = model.cursor { model.cursor = (try? cursor.removing([projectId])) ?? nil }
        onChange(model)
        flush()
        return .removed
    }

    /// Saves now what is held.
    public func flush() {
        cancelDeferredSave()
        write(persistedNow())
    }

    // MARK: Saving

    private func completeProjectIds() -> Set<String> {
        Set(model.projects.filter { $0.value.isResumable && staging.pending[$0.key] == nil }.keys)
    }

    private func persistedNow() -> PersistedHost {
        persistedBuildCount += 1
        return PersistedHost(host: model, staging: staging, savedAt: clock.now())
    }

    /// The first save, and any after a quiet interval, happens at once; the ones in between become one save at the end of
    /// the interval, with the newest cursor and models. Nothing is built until the write.
    private func scheduleSave() {
        let now = clock.now()
        guard let last = lastSaveAt, now.timeIntervalSince(last) < HostStoreEngine.saveIntervalSeconds else {
            cancelDeferredSave()
            write(persistedNow())
            return
        }
        saveDirty = true
        guard saveTask == nil else { return }
        let delay = HostStoreEngine.saveIntervalSeconds - now.timeIntervalSince(last)
        saveTask = Task { await self.saveAfter(delay) }
    }

    private func saveAfter(_ delay: TimeInterval) async {
        do {
            try await clock.sleep(seconds: delay)
        } catch {
            return
        }
        guard !Task.isCancelled else { return }
        saveTask = nil
        guard saveDirty else { return }
        write(persistedNow())
    }

    private func cancelDeferredSave() {
        saveTask?.cancel()
        saveTask = nil
        saveDirty = false
    }

    private func write(_ pair: PersistedHost) {
        saveDirty = false
        var saved = pair
        saved.savedAt = clock.now()
        lastSaveAt = saved.savedAt
        do {
            try persistence.save(saved)
            lastSaveError = nil
        } catch {
            lastSaveError = "\(error)"
        }
    }
}
