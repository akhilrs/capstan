import Foundation

/// Runs the history reads of the project overview and keeps their display-only store. It reads the host's model, but
/// nothing it holds goes back into the store engine, except the folded sections the person sets.
@MainActor
public final class OverviewController {
    private let store: HostStoreEngine
    private let runner: any ProcessRunner
    private let config: HostConfig
    private let clock: any SessionClock

    private var trigger = RecentTrigger()
    private var histories: [String: RecentHistoryState] = [:]
    private var host: HostModel?
    private var liveness: ConnectionLiveness = .unknown
    private var selectedProjectId: String?

    public init(
        store: HostStoreEngine, runner: any ProcessRunner, config: HostConfig,
        clock: any SessionClock = SystemClock()
    ) {
        self.store = store
        self.runner = runner
        self.config = config
        self.clock = clock
    }

    // MARK: Reading

    public func freshness(_ projectId: String) -> ProjectFreshness {
        guard let project = host?.projects[projectId] else { return ProjectFreshness.unknown }
        return ProjectFreshness(project: project, liveness: liveness)
    }

    public func history(_ projectId: String) -> RecentHistoryState {
        histories[projectId] ?? RecentHistoryState(projectId: projectId)
    }

    public func folded(_ projectId: String) -> Set<String> {
        host?.projects[projectId]?.folded ?? []
    }

    // MARK: Changes

    public func setFolded(_ projectId: String, _ folded: Set<String>) {
        host?.projects[projectId]?.foldedSections = folded.isEmpty ? nil : folded.sorted()
        let store = self.store
        Task { await store.setFolded(projectId: projectId, folded) }
    }

    /// Takes the host's model as the engine reports it, and reads the history of the selected project when its
    /// position has just become known.
    public func update(host model: HostModel) async {
        host = model
        guard let project = selectedProjectId else { return }
        await handle(.selected(projectId: project))
    }

    public func update(liveness state: ConnectionLiveness) {
        liveness = state
    }

    /// Pulls the model and the liveness from the engine.
    public func refreshFromStore() async {
        let model = await store.current()
        let state = await store.connectionLiveness(intervalSeconds: Double(config.pollMs) / 1000)
        liveness = state
        await update(host: model)
    }

    /// The live events of a project, as the engine reads them from the stream. They join the history by their key.
    public func noteLive(projectId: String, events: [FeedEvent]) {
        guard let epoch = host?.projects[projectId]?.epoch else { return }
        var state = history(projectId)
        state.merge(live: events, epoch: epoch, at: clock.now())
        histories[projectId] = state
    }

    /// Feeds one event to the trigger and runs the reads it asks for.
    public func handle(_ event: RecentTriggerEvent) async {
        let now = clock.now()
        switch event {
        case .connected:
            for id in Array(histories.keys) { histories[id]?.connected(at: now) }
        case .selected(let projectId):
            selectedProjectId = projectId
        case .resynced(let projectId, _):
            var state = history(projectId)
            state.resync(at: now)
            histories[projectId] = state
        default:
            break
        }
        let projects = host?.projects ?? [:]
        let calls = trigger.handle(event, now: now) { (projectId: String) -> RecentPosition? in
            guard let project = projects[projectId], project.availability == .ready, project.snapshotComplete,
                  let epoch = project.epoch, let head = project.head else { return nil }
            return RecentPosition(epoch: epoch, head: head)
        }
        for call in calls { await run(call) }
    }

    private func run(_ call: RecentCall) async {
        var state = history(call.projectId)
        if state.connectedAt == nil { state.connected(at: clock.now()) }
        state.begin(epoch: call.epoch, head: call.head)
        histories[call.projectId] = state
        let result = await call.run(runner: runner, config: config, clock: clock)
        var answered = history(call.projectId)
        answered.apply(result, at: clock.now())
        histories[call.projectId] = answered
        if result == .rateLimited {
            trigger.limit(projectId: call.projectId, until: answered.retryNotBefore ?? clock.now())
        }
        let more = trigger.handle(.answered(projectId: call.projectId, result)) { (_: String) -> RecentPosition? in nil }
        for next in more { await run(next) }
    }
}
