import Foundation

/// Holds the model of one host: reads what a `HostSession` delivers, keeps the model and saves it for the next run.
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
    private let onLiveEvents: (@Sendable (String, [FeedEvent]) -> Void)?
    private let monotonic: any MonotonicClock

    private var liveness = LivenessTracker()
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
        onConnection: (@Sendable (ConnectionState) -> Void)? = nil,
        onLiveEvents: (@Sendable (String, [FeedEvent]) -> Void)? = nil,
        monotonic: any MonotonicClock = ContinuousMonotonicClock()
    ) {
        self.onLiveEvents = onLiveEvents
        self.monotonic = monotonic
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

    /// Whether the host sent a line within three of the given intervals, by the monotonic clock.
    public func connectionLiveness(intervalSeconds: TimeInterval) -> ConnectionLiveness {
        liveness.state(at: monotonic.seconds(), intervalSeconds: intervalSeconds)
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
        liveness.noteLine(at: monotonic.seconds())
        var liveEvents: [(String, FeedEvent)] = []
        var refresh = Set<String>()
        var urgent = Set<String>()
        var notify: [NotifyCandidate] = []
        var cursorSeen = false
        for line in lines {
            let output = StoreReducer.apply(line, to: &model, staging: &staging, now: now)
            refresh.formUnion(output.refresh)
            urgent.formUnion(output.urgent)
            notify.append(contentsOf: output.notify)
            if case .cursor = line { cursorSeen = true }
            if case .event(let projectId, let event) = line { liveEvents.append((projectId, event)) }
        }
        onChange(model)
        // The overview's history reads the events; nothing in it comes back into the store.
        if let onLiveEvents {
            for projectId in Set(liveEvents.map { $0.0 }).sorted() {
                onLiveEvents(projectId, liveEvents.filter { $0.0 == projectId }.map { $0.1 })
            }
        }
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
        switch state {
        case .connecting:
            liveness.reset()
        case .reconnecting, .stopped:
            liveness.reset()
            // The stream ended: a project that waited for a read keeps what it knew and notes the failure.
            let before = model
            StoreReducer.noteRefreshFailure(in: &model, now: clock.now())
            if model != before { onChange(model) }
        case .live, .noHeartbeat:
            break
        }
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

    /// Records the overview sections the person folded for a project; an empty set unfolds all.
    public func setFolded(projectId: String, _ folded: Set<String>) {
        guard var project = model.projects[projectId] else { return }
        project.foldedSections = folded.isEmpty ? nil : folded.sorted()
        model.projects[projectId] = project
        onChange(model)
        scheduleSave()
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
