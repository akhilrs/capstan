import AppKit
import CapstanAppCore
import CapstanKit
import Foundation
import Observation
import os

/// What the windows read and call: the hosts, the models their engines keep, the selection and the decisions.
/// Sessions and engines are made in `start()`, never in an initializer.
@Observable @MainActor
final class AppModel {
    static let selectionKey = "selection.v1"
    static let settingsKey = "settings.v1"

    var hosts: [HostConfig]
    var hostModels: [UUID: HostModel] {
        didSet { modelVersion += 1 }
    }
    var connection: [UUID: ConnectionState]
    private var connectionNumber: [UUID: Int] = [:]
    var capabilities: [UUID: HostCapabilities]
    var selection: SidebarSelection {
        didSet {
            guard selection != oldValue else { return }
            selectedAgent = nil
            selectedPlan = nil
            if !restoring {
                cancelPendingRow()
                restoredSelection = nil
            }
            saveSelection()
        }
    }
    var selectedRow: InboxRowID?
    var inboxKind: WaitingKind?
    var inboxSearch = ""
    var agentFilter = AgentFilter.all
    var planFilter = PlanFilter.active
    var selectedAgent: String?
    var selectedPlan: PlanRef?
    var hostsFocus: HostsFocus?
    private(set) var busy = BusyTracker()
    private(set) var settings: AppSettings
    @ObservationIgnored var notifications: NotificationCoordinator!
    var presented: AppSheet?
    @ObservationIgnored var openMainWindow: (@MainActor () -> Void)?

    @ObservationIgnored private let defaults: UserDefaults?
    @ObservationIgnored private let directory: URL?
    @ObservationIgnored private let pendingStore: PendingStore?
    @ObservationIgnored private let decider: Decider?
    @ObservationIgnored private var sessions: [UUID: HostSession] = [:]
    @ObservationIgnored private var engines: [UUID: HostStoreEngine] = [:]
    @ObservationIgnored private var generations: [UUID: Int] = [:]
    @ObservationIgnored private var probes: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored private var forwards: [BusyKey: ForwardRecord] = [:]
    @ObservationIgnored private var started = false
    /// The saved selection until the models it needs are known; hosts without a saved file are in `unloaded`.
    @ObservationIgnored private var restoredSelection: SidebarSelection?
    @ObservationIgnored private var unloaded: Set<UUID> = []
    @ObservationIgnored private var restoring = false
    @ObservationIgnored private var pendingRow: PendingRow?
    /// Counts the changes of `hostModels`; what is derived from the models is kept for one count.
    @ObservationIgnored private var modelVersion = 0
    @ObservationIgnored private var derived = DerivedReads()
    @ObservationIgnored private var pendingModels: [UUID: (model: HostModel, generation: Int)] = [:]
    @ObservationIgnored private var modelFlushScheduled = false
    @ObservationIgnored private var lifecycle: Task<Void, Never>?
    @ObservationIgnored private var pendingTask: Task<Void, Never>?

    // MARK: Creating

    /// The app's model: loads the hosts file, the settings and the selection. It makes no session, engine or closure.
    init() {
        let defaults = UserDefaults.standard
        let directory = AppModel.supportDirectory()
        let pending = PendingStore(url: directory.appendingPathComponent("pending.json", isDirectory: false))
        let saved = defaults.data(forKey: AppModel.selectionKey).flatMap {
            try? JSONDecoder().decode(SidebarSelection.self, from: $0)
        }
        self.defaults = defaults
        self.directory = directory
        self.pendingStore = pending
        self.decider = Decider(runner: LiveProcessRunner(), pending: pending, clock: SystemClock())
        self.hosts = HostsFile(directory: directory).load()
        self.hostModels = [:]
        self.connection = [:]
        self.capabilities = [:]
        self.selection = saved ?? .inbox
        self.restoredSelection = saved
        self.settings = defaults.data(forKey: AppModel.settingsKey).flatMap {
            try? JSONDecoder().decode(AppSettings.self, from: $0)
        } ?? AppSettings()
        self.notifications = NotificationCoordinator(model: self)
        AppModel.apply(appearance: settings.appearance)
    }

    /// Sets the look of every window, the Settings window and the menu bar extra included.
    static func apply(appearance: AppearanceChoice) {
        switch appearance {
        case .system: NSApp?.appearance = nil
        case .light: NSApp?.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp?.appearance = NSAppearance(named: .darkAqua)
        }
    }

    /// A model for previews and tests: nothing is read or written and nothing runs.
    init(preview seed: PreviewSeed) {
        self.defaults = nil
        self.directory = nil
        self.pendingStore = nil
        self.decider = nil
        self.hosts = seed.hosts
        self.hostModels = seed.hostModels
        self.connection = seed.connection
        self.capabilities = seed.capabilities
        self.selection = .inbox
        self.settings = seed.settings
        self.notifications = NotificationCoordinator(model: self)
    }

    private static func supportDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        let url = base.appendingPathComponent("Capstan", isDirectory: true)
        try? FileManager.default.createDirectory(
            at: url, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        return url
    }

    // MARK: Running

    /// Loads what the engines saved, resolves the saved selection against it, then starts a session and an engine per
    /// enabled host. The app delegate calls it once.
    func start() {
        guard !started, directory != nil else { return }
        started = true
        AppModel.apply(appearance: settings.appearance)
        enqueue { [self] in
            guard started else { return }
            await launch()
        }
    }

    /// The lifecycle rule: every creation and stop of a session or engine (start, stop, add, update, remove, restart)
    /// runs inside `enqueue`, one operation after the other, and an operation reads `hosts` and `started` when it
    /// runs, never values captured when it was queued.
    private func enqueue(_ op: @escaping @MainActor () async -> Void) {
        let prev = lifecycle
        lifecycle = Task {
            await prev?.value
            await op()
        }
    }

    private func launch() async {
        guard let directory else { return }
        let persistence = FilePersistence(directory: directory)
        for config in hosts {
            guard let saved = persistence.load(hostId: config.id) else {
                unloaded.insert(config.id)
                continue
            }
            var model = HostModel(hostId: config.id)
            model.projects = saved.projects
            model.cursor = saved.cursor
            model.forwards = saved.forwards
            hostModels[config.id] = model
            for (projectId, records) in saved.forwards {
                for record in records {
                    forwards[BusyKey(hostId: config.id, projectId: projectId, requestId: record.requestId)] = record
                }
            }
        }
        resolveRestoredSelection()
        for config in hosts where config.enabled {
            await startHost(config)
        }
        notifications.launched()
    }

    /// Stops the sessions and cancels the waiting tasks.
    func stop() {
        started = false
        cancelPendingRow()
        enqueue { [self] in
            for id in Set(sessions.keys).union(engines.keys) {
                await teardownHost(id)
            }
            for host in hosts { connection[host.id] = .stopped(.user) }
        }
    }

    /// Queues the same teardown as `stop()` and returns once the lifecycle chain has drained, so every engine has
    /// flushed its cursor, forwards and model before the caller goes on. The app delegate awaits it on quit.
    func stopAndWait() async {
        stop()
        await lifecycle?.value
    }

    private func startHost(_ config: HostConfig) async {
        guard let directory else { return }
        let id = config.id
        let generation = (generations[id] ?? 0) + 1
        generations[id] = generation
        let numbering = StateNumbering()
        connectionNumber[id] = 0
        let engine = HostStoreEngine(
            hostId: id, persistence: FilePersistence(directory: directory), clock: SystemClock(),
            onChange: { [weak self] model in
                Task { @MainActor [weak self] in self?.applyModel(model, generation: generation) }
            },
            onNotify: { [weak self] candidates in
                Task { @MainActor [weak self] in
                    guard let self, self.generations[id] == generation else { return }
                    self.notifications.handle(candidates)
                }
            },
            onRefresh: { [weak self] projectIds in
                Task { @MainActor [weak self] in
                    guard let self, self.generations[id] == generation, let session = self.sessions[id] else { return }
                    await session.refresh(projectIds: projectIds)
                }
            },
            onConnection: { [weak self] state in
                // Numbered here, in the engine's order, because separate tasks may reach the main actor out of order.
                let number = numbering.next()
                Task { @MainActor [weak self] in
                    self?.applyConnection(state, number: number, hostId: id, generation: generation)
                }
            })
        let session = HostSession(
            config: config, runner: LiveProcessRunner(), consumer: engine, clock: SystemClock(),
            diagnostic: { what, detail in
                // The detail can carry host stderr text: it stays private.
                AppModel.log.notice(
                    "\(config.displayName, privacy: .public): \(what, privacy: .public) \(detail ?? "", privacy: .private)")
            })
        engines[id] = engine
        sessions[id] = session
        connection[id] = .connecting
        probe(config, generation: generation)
        await session.start()
    }

    /// Stops the session of a host, saves its engine and forgets both; the models stay. Runs inside `enqueue`.
    private func teardownHost(_ id: UUID) async {
        generations[id] = (generations[id] ?? 0) + 1
        probes[id]?.cancel()
        probes[id] = nil
        let session = sessions.removeValue(forKey: id)
        let engine = engines.removeValue(forKey: id)
        await session?.stop()
        await engine?.flush()
    }

    private func probe(_ config: HostConfig, generation: Int) {
        let id = config.id
        probes[id]?.cancel()
        probes[id] = Task { [weak self] in
            let found = await probeCapabilities(config, LiveProcessRunner())
            guard !Task.isCancelled, let self, self.generations[id] == generation else { return }
            self.capabilities[id] = found
        }
    }

    /// Keeps the newest model of each host and applies them together on the next turn of the main actor, so the many
    /// batches of a first load make one update of the views, not one each.
    private func applyModel(_ model: HostModel, generation: Int) {
        guard generations[model.hostId] == generation else { return }
        pendingModels[model.hostId] = (model, generation)
        guard !modelFlushScheduled else { return }
        modelFlushScheduled = true
        Task { @MainActor [weak self] in self?.flushModels() }
    }

    private func flushModels() {
        modelFlushScheduled = false
        let pending = pendingModels
        pendingModels = [:]
        for (id, entry) in pending where generations[id] == entry.generation && hosts.contains(where: { $0.id == id }) {
            hostModels[id] = entry.model
            unloaded.remove(id)
        }
        resolveRestoredSelection()
        resolvePendingRow(final: false)
    }

    private func applyConnection(_ state: ConnectionState, number: Int, hostId: UUID, generation: Int) {
        guard generations[hostId] == generation, number > (connectionNumber[hostId] ?? 0) else { return }
        connectionNumber[hostId] = number
        var reconnected = false
        if case .live = state, case .reconnecting? = connection[hostId] { reconnected = true }
        connection[hostId] = state
        if reconnected, let config = hosts.first(where: { $0.id == hostId }) {
            probe(config, generation: generation)
        }
    }

    // MARK: Selection

    private func saveSelection() {
        guard let defaults, let data = try? JSONEncoder().encode(selection) else { return }
        defaults.set(data, forKey: AppModel.selectionKey)
    }

    /// Resolves the saved selection once the models of its host are known; a host with no saved file keeps it until
    /// its first change.
    private func resolveRestoredSelection() {
        guard let saved = restoredSelection, selection == saved else {
            restoredSelection = nil
            return
        }
        switch saved {
        case .inbox, .hosts, .messages:
            restoredSelection = nil
            return
        case .project(let hostId, _), .agents(let hostId, _), .plans(let hostId, _):
            if unloaded.contains(hostId), hosts.contains(where: { $0.id == hostId && $0.enabled }) { return }
        }
        restoring = true
        selection = saved.resolved(against: Array(hostModels.values))
        restoring = false
        restoredSelection = nil
    }

    func showProject(hostId: UUID, projectId: String, kind: WaitingKind?) {
        selection = .project(hostId: hostId, projectId: projectId)
        inboxKind = kind
        selectedRow = nil
    }

    /// Opens the main window on the inbox with the row selected. A row that has not arrived yet is waited for.
    func showMain(row: InboxRowID?) {
        selection = .inbox
        inboxKind = nil
        if let row {
            if allRowIDs().contains(row) {
                cancelPendingRow()
                selectedRow = row
            } else {
                cancelPendingRow()
                selectedRow = nil
                pendingRow = PendingRow(row: row, since: Date())
                pendingTask = Task { [weak self] in
                    try? await Task.sleep(for: .seconds(PendingRow.waitSeconds))
                    guard !Task.isCancelled else { return }
                    self?.resolvePendingRow(final: true)
                }
            }
        }
        NSApp.activate(ignoringOtherApps: true)
        // Only a window that can be key: glass popovers and panels refuse it and log an error.
        if let window = NSApp.windows.first(where: {
            $0.identifier?.rawValue.contains("main") == true && $0.canBecomeKey && !($0 is NSPanel)
        }) {
            window.makeKeyAndOrderFront(nil)
        } else {
            openMainWindow?()
        }
    }

    private func allRowIDs() -> Set<InboxRowID> {
        var ids = Set<InboxRowID>()
        for host in hostModels.values {
            for project in host.projects.values {
                for key in project.waiting.keys {
                    ids.insert(InboxRowID(hostId: host.hostId, projectId: project.projectId, key: key))
                }
            }
        }
        return ids
    }

    /// Selects the awaited row when it has arrived; when the time is out, leaves the inbox selected without a row.
    private func resolvePendingRow(final: Bool) {
        guard let pending = pendingRow else { return }
        switch pending.resolve(rows: allRowIDs(), now: Date()) {
        case .select:
            selectedRow = pending.row
            cancelPendingRow()
        case .expire:
            cancelPendingRow()
        case .wait:
            if final { cancelPendingRow() }
        }
    }

    private func cancelPendingRow() {
        pendingTask?.cancel()
        pendingTask = nil
        pendingRow = nil
    }

    // MARK: Reading

    /// The groups of every waiting item, whatever the selection; kept until the models change.
    private func baseInbox() -> [InboxGroup] {
        let models = hostModels  // read here, so a view that calls this is told of a change even when the answer is kept
        if let kept = derived.base, kept.version == modelVersion { return kept.groups }
        let groups = CapstanKit.inbox(Array(models.values))
        derived.base = (modelVersion, groups)
        return groups
    }

    /// The groups the inbox shows for the selection, the kind filter and the search.
    func inbox() -> [InboxGroup] {
        scope(kind: inboxKind).apply(baseInbox())
    }

    /// The same groups as the screen lists them: merged and conflicted integrations apart. Kept until the models, the
    /// selection, the kind filter or the search change.
    func displayGroups() -> [DisplayGroup] {
        let key = DerivedReads.GroupsKey(version: modelVersion, selection: selection, kind: inboxKind, search: inboxSearch)
        _ = hostModels
        if let kept = derived.display, kept.key == key { return kept.groups }
        let groups = InboxLayout.groups(inbox())
        derived.display = (key, groups)
        return groups
    }

    /// The number of rows the selection and the search leave, whatever the kind filter: the "All" count.
    func inboxTotal() -> Int {
        scope(kind: nil).apply(baseInbox()).reduce(0) { $0 + $1.count }
    }

    private func scope(kind: WaitingKind?) -> InboxScope {
        var scope = InboxScope(kind: kind, search: inboxSearch)
        if case .project(let hostId, let projectId) = selection {
            scope.hostId = hostId
            scope.projectId = projectId
        }
        return scope
    }

    /// The rows the list shows now, for keeping the inspector on a row that is still there.
    var visibleRowIDs: [InboxRowID] {
        inbox().flatMap { $0.rows.map(\.id) }
    }

    var sidebarCounts: SidebarCounts {
        let models = hostModels
        if let kept = derived.counts, kept.version == modelVersion { return kept.counts }
        let counts = SidebarCounts.make(Array(models.values))
        derived.counts = (modelVersion, counts)
        return counts
    }

    var badgeCount: Int {
        let models = hostModels
        if let kept = derived.badge, kept.version == modelVersion { return kept.count }
        let count = CapstanKit.badgeCount(Array(models.values))
        derived.badge = (modelVersion, count)
        return count
    }

    func project(_ hostId: UUID, _ projectId: String) -> ProjectModel? {
        hostModels[hostId]?.projects[projectId]
    }

    func item(_ row: InboxRowID) -> StoredWaiting? {
        project(row.hostId, row.projectId)?.waiting[row.key]
    }

    func detail(_ row: InboxRowID) -> ItemDetail? {
        project(row.hostId, row.projectId)?.details[row.key]
    }

    func forwardShown(_ row: InboxRowID) -> Bool {
        guard let item = item(row) else { return false }
        let key = BusyKey(hostId: row.hostId, projectId: row.projectId, requestId: item.requestId)
        return ForwardPill.shown(item: item, record: forwards[key])
    }

    /// Whether a verb is offered for the row. Without a row (agent and run verbs) the host of the current view counts.
    func availability(_ verb: DecideVerb, row: InboxRowID?, option: PromptOption?) -> DecideAvailability {
        let hostId = row?.hostId ?? currentHostId()
        guard let hostId, let caps = capabilities[hostId] else { return .hidden("The host has not been checked yet") }
        let stored = row.flatMap { item($0) }
        return CapstanKit.availability(verb, capabilities: caps, item: stored.flatMap(AppModel.wireItem), option: option)
    }

    private func currentHostId() -> UUID? {
        switch selection {
        case .project(let hostId, _), .agents(let hostId, _), .plans(let hostId, _): return hostId
        case .hosts: return hostsFocus?.hostId
        case .inbox, .messages: return nil
        }
    }

    private nonisolated static let log = Logger(subsystem: "capstan", category: "AppModel")

    private static func wireItem(_ stored: StoredWaiting) -> WaitingItem? {
        do {
            return try WaitingItem(
                json: .object([
                    "key": .string(stored.key), "requestId": .string(stored.requestId),
                    "kind": .string(stored.kind.wireName), "since": .string(stored.since),
                    "summary": .string(stored.summary.raw), "contentHash": .string(stored.contentHash),
                    "needsTerminal": .bool(stored.needsTerminal),
                ]))
        } catch {
            log.error("stored item could not be read as a wire item: \(String(describing: error), privacy: .public)")
            return nil
        }
    }

    /// The projects of a host that are ready to be read again.
    func readyProjects(hostId: UUID) -> Set<String> {
        Set((hostModels[hostId]?.projects ?? [:]).filter { $0.value.availability == .ready }.keys)
    }

    // MARK: Deciding

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

    // MARK: Hosts

    func refresh(hostId: UUID, projectIds: Set<String>) {
        guard let session = sessions[hostId], !projectIds.isEmpty else { return }
        Task { await session.refresh(projectIds: projectIds) }
    }

    func restart(hostId: UUID) {
        guard sessions[hostId] != nil else { return }
        connection[hostId] = .connecting
        enqueue { [self] in
            guard let session = sessions[hostId] else { return }
            await session.restart()
        }
    }

    /// Cmd-R: reads again the ready projects the current view shows.
    func refreshSelection() {
        switch selection {
        case .inbox, .messages:
            for host in hosts { refresh(hostId: host.id, projectIds: readyProjects(hostId: host.id)) }
        case .project(let hostId, let projectId), .agents(let hostId, let projectId), .plans(let hostId, let projectId):
            refresh(hostId: hostId, projectIds: readyProjects(hostId: hostId).intersection([projectId]))
        case .hosts:
            guard let focus = hostsFocus else { return }
            refresh(hostId: focus.hostId, projectIds: readyProjects(hostId: focus.hostId))
        }
    }

    func addHost(_ config: HostConfig) {
        hosts.append(config)
        saveHosts()
        enqueue { [self] in
            guard started, sessions[config.id] == nil,
                let current = hosts.first(where: { $0.id == config.id }), current.enabled
            else { return }
            await startHost(current)
        }
    }

    func updateHost(_ config: HostConfig) {
        guard let index = hosts.firstIndex(where: { $0.id == config.id }) else { return }
        hosts[index] = config
        saveHosts()
        let id = config.id
        enqueue { [self] in
            await teardownHost(id)
            guard started, let current = hosts.first(where: { $0.id == id }), current.enabled else {
                if hosts.contains(where: { $0.id == id }) {
                    connection[id] = .stopped(.user)
                    capabilities[id] = nil
                }
                return
            }
            await startHost(current)
        }
    }

    func removeHost(_ id: UUID) {
        if pendingRow?.row.hostId == id { cancelPendingRow() }
        hosts.removeAll { $0.id == id }
        saveHosts()
        enqueue { [self] in
            // The engine has flushed and stopped when this returns, so nothing writes its file again.
            await teardownHost(id)
            hostModels[id] = nil
            connection[id] = nil
            capabilities[id] = nil
            forwards = forwards.filter { $0.key.hostId != id }
            unloaded.remove(id)
            if selectedRow?.hostId == id { selectedRow = nil }
            if hostsFocus?.hostId == id { hostsFocus = nil }
            selection = selection.resolved(against: Array(hostModels.values))
            if let directory {
                try? FilePersistence(directory: directory).delete(hostId: id)
            }
            await pendingStore?.remove(hostId: id)
        }
    }

    private func saveHosts() {
        guard let directory else { return }
        try? HostsFile(directory: directory).save(hosts)
    }

    /// The steps of a first connection test, for the sheet; nothing of it is kept.
    func check(_ config: HostConfig) -> AsyncStream<HostCheck> {
        checkHost(config, LiveProcessRunner())
    }

    // MARK: Settings

    func updateSettings(_ new: AppSettings) {
        settings = new
        if let defaults, let data = try? JSONEncoder().encode(new) {
            defaults.set(data, forKey: AppModel.settingsKey)
        }
        AppModel.apply(appearance: new.appearance)
        notifications.settingsChanged(new)
    }
}

/// A plan of a project of a host, as the Plans screen selects it.
struct PlanRef: Hashable {
    let hostId: UUID
    let projectId: String
    let planId: String
}

/// Hands out increasing numbers from any thread; a connection state carries its number to the main actor.
private final class StateNumbering: @unchecked Sendable {
    private let lock = NSLock()
    private var last = 0

    func next() -> Int {
        lock.withLock {
            last += 1
            return last
        }
    }
}

/// What is worked out from the models for the views, kept for one version of the models.
private struct DerivedReads {
    struct GroupsKey: Equatable {
        let version: Int
        let selection: SidebarSelection
        let kind: WaitingKind?
        let search: String
    }

    var base: (version: Int, groups: [InboxGroup])?
    var display: (key: GroupsKey, groups: [DisplayGroup])?
    var counts: (version: Int, counts: SidebarCounts)?
    var badge: (version: Int, count: Int)?
}
