import CapstanAppCore
import CapstanKit
import Foundation
import Observation

/// What the overview keeps outside the host models: one controller per host, the outcome the person selected, and a
/// count of changes that the views watch. The controllers decide and run the history reads; nothing here decides.
@Observable @MainActor
final class OverviewStore {
    /// How a connection report changed what the overview knows.
    enum ConnectionChange {
        case connected, disconnected, unchanged
    }

    /// Changes whenever a controller's state may have changed; a view that reads it draws again.
    private(set) var version = 0
    /// The id of the outcome line the inspector shows.
    var selectedOutcomeId: String?

    /// The runner the history reads use: the app's one `SSHRunner`; nil in previews.
    @ObservationIgnored private let runner: (any ProcessRunner)?
    /// The project the overview screen shows; nil when it is closed.
    @ObservationIgnored var shownProject: ShownProject?
    @ObservationIgnored private var controllers: [UUID: (engine: ObjectIdentifier, controller: OverviewController)] = [:]
    @ObservationIgnored private var live: Set<UUID> = []
    @ObservationIgnored private var epochs: [String: String] = [:]
    @ObservationIgnored private var inFlight = 0
    @ObservationIgnored private var tail: Task<Void, Never>?
    @ObservationIgnored private var ticker: Task<Void, Never>?

    struct ShownProject: Equatable {
        let hostId: UUID
        let projectId: String
    }

    init(runner: (any ProcessRunner)?) {
        self.runner = runner
    }

    func controller(_ hostId: UUID) -> OverviewController? {
        controllers[hostId]?.controller
    }

    /// The controller of a host; a new engine (the host was restarted) gets a new controller.
    func controller(_ hostId: UUID, engine: HostStoreEngine, config: HostConfig) -> OverviewController? {
        let identity = ObjectIdentifier(engine)
        if let held = controllers[hostId], held.engine == identity { return held.controller }
        guard let runner else { return nil }
        let made = OverviewController(store: engine, runner: runner, config: config)
        controllers[hostId] = (engine: identity, controller: made)
        return made
    }

    func noteConnection(live isLive: Bool, hostId: UUID) -> ConnectionChange {
        if isLive {
            return live.insert(hostId).inserted ? .connected : .unchanged
        }
        return live.remove(hostId) != nil ? .disconnected : .unchanged
    }

    /// True when the project's epoch is not the one seen before: its ledger was replaced.
    func epochChanged(hostId: UUID, projectId: String, to epoch: String?) -> Bool {
        guard let epoch else { return false }
        let key = hostId.uuidString + "/" + projectId
        let before = epochs[key]
        epochs[key] = epoch
        guard let before else { return false }
        return before != epoch
    }

    func touch() {
        version += 1
    }

    /// Runs work one piece after the other. While any is running the views are told every quarter of a second, so
    /// the loading state of a read shows while the read is under way.
    func run(_ work: @escaping @MainActor () async -> Void) {
        inFlight += 1
        startTicker()
        let previous = tail
        tail = Task { @MainActor [weak self] in
            await previous?.value
            await work()
            self?.finished()
        }
    }

    private func finished() {
        inFlight -= 1
        version += 1
    }

    private func startTicker() {
        guard ticker == nil else { return }
        ticker = Task { @MainActor [weak self] in
            while true {
                guard let self, self.inFlight > 0 else { break }
                self.version += 1
                try? await Task.sleep(for: .milliseconds(250))
            }
            self?.ticker = nil
        }
    }
}

extension AppModel {
    /// The overview of a project as the views draw it; nil while the app does not hold the project.
    func overviewContent(hostId: UUID, projectId: String) -> OverviewModel? {
        _ = overview.version
        guard let held = project(hostId, projectId) else { return nil }
        let controller = overview.controller(hostId)
        let freshness: ProjectFreshness
        if let controller {
            freshness = controller.freshness(projectId)
        } else {
            freshness = ProjectFreshness(project: held, liveness: .unknown)
        }
        let history = controller?.history(projectId) ?? RecentHistoryState(projectId: projectId)
        let folded = controller?.folded(projectId) ?? held.folded
        return OverviewModel.make(
            project: held, freshness: freshness, history: history, folded: folded, now: Date())
    }

    /// Called when a host's connection state changes. A host that became live starts the history read of the selected
    /// project; one that is no longer live lets the next connection read it again.
    func overviewConnection(_ state: ConnectionState, hostId: UUID, engine: HostStoreEngine?) {
        guard let engine, let config = hosts.first(where: { $0.id == hostId }),
              let controller = overview.controller(hostId, engine: engine, config: config) else { return }
        let isLive: Bool
        switch state {
        case .live: isLive = true
        default: isLive = false
        }
        switch overview.noteConnection(live: isLive, hostId: hostId) {
        case .connected:
            let picked = selectedProjectId(of: hostId)
            overview.run {
                await controller.refreshFromStore()
                await controller.handle(.connected(hostId: hostId))
                if let picked { await controller.handle(.selected(projectId: picked)) }
            }
        case .disconnected:
            overview.run { await controller.handle(.disconnected(hostId: hostId)) }
        case .unchanged:
            break
        }
    }

    /// Called when the project screen appears or closes: a project that is shown is read once per connection.
    func overviewShow(hostId: UUID, projectId: String?) {
        overview.selectedOutcomeId = nil
        overview.shownProject = projectId.map { OverviewStore.ShownProject(hostId: hostId, projectId: $0) }
        guard let projectId, let controller = overview.controller(hostId) else { return }
        overview.run {
            await controller.refreshFromStore()
            await controller.handle(.selected(projectId: projectId))
        }
    }

    /// Called by pull to refresh, and by Retry under Recent outcomes.
    func overviewRefreshed() {
        guard let shown = overview.shownProject, let controller = overview.controller(shown.hostId) else { return }
        let projectId = shown.projectId
        overview.run {
            await controller.refreshFromStore()
            await controller.handle(.refreshed(projectId: projectId))
        }
    }

    /// Called by the live stream for the events of a project. They join the history by their key.
    func overviewLive(hostId: UUID, projectId: String, events: [FeedEvent]) {
        guard let controller = overview.controller(hostId) else { return }
        controller.noteLive(projectId: projectId, events: events)
        overview.touch()
    }

    /// Called by the overview while it shows a project: takes the host's model and the liveness, and tells the
    /// controller when the project's ledger was replaced.
    func overviewFeed(hostId: UUID, projectId: String) {
        guard let controller = overview.controller(hostId) else { return }
        let replaced = overview.epochChanged(hostId: hostId, projectId: projectId, to: project(hostId, projectId)?.epoch)
        let head = project(hostId, projectId)?.head
        overview.run {
            await controller.refreshFromStore()
            if replaced, let head { await controller.handle(.resynced(projectId: projectId, head: head)) }
        }
    }

    /// Folds or unfolds a section; the choice is remembered for the project.
    func overviewToggle(hostId: UUID, projectId: String, section: String) {
        guard let controller = overview.controller(hostId) else { return }
        var folded = controller.folded(projectId)
        if folded.contains(section) {
            folded.remove(section)
        } else {
            folded.insert(section)
        }
        controller.setFolded(projectId, folded)
        overview.touch()
    }

    private func selectedProjectId(of hostId: UUID) -> String? {
        guard let shown = overview.shownProject, shown.hostId == hostId else { return nil }
        return shown.projectId
    }
}
