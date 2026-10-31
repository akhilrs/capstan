import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation
import Observation
import os

/// The tabs of the app.
enum PhoneTab: Hashable {
    case waiting, agents, plans, messages, hosts
}

/// A screen pushed on a tab's stack.
enum PhoneRoute: Hashable {
    case item(InboxRowID)
    case project(hostId: UUID, projectId: String)
    case agent(hostId: UUID, projectId: String, agentId: String)
    case plan(PlanRef)
    case conversation(hostId: UUID, projectId: String)
    case host(UUID)
}

/// A sheet over the tabs.
enum PhoneSheet: Identifiable, Hashable {
    case addHost
    case editHost(UUID)
    case trust(UUID)
    /// The saved host keys cannot be read: reset them all (scr-hosts).
    case resetPins
    case settings

    var id: Self { self }
}

/// The selected tab, one navigation path per tab and the presented sheet.
struct PhoneNavigation: Equatable {
    var tab: PhoneTab = .waiting
    var paths: [PhoneTab: [PhoneRoute]] = [:]
    var sheet: PhoneSheet?

    func path(_ tab: PhoneTab) -> [PhoneRoute] {
        paths[tab] ?? []
    }

    /// Selects a tab and replaces its path.
    mutating func show(_ tab: PhoneTab, _ path: [PhoneRoute]) {
        self.tab = tab
        paths[tab] = path
    }
}

/// A plan of a project of a host, as the Plans screen selects it.
struct PlanRef: Hashable {
    let hostId: UUID
    let projectId: String
    let planId: String
}

/// The phone's key as Settings and onboarding show it.
enum PhoneKeyState: Equatable {
    /// No key yet, or a restored phone whose enclave key stayed behind: "This iPhone needs a new key".
    case none
    case present(kind: PhoneKeyKind, publicLine: String, fingerprint: String)
    /// The key could not be read (or no enclave, in the Simulator); the text says why.
    case unavailable(String)
}

/// What every screen reads and calls: the hosts, the models their engines keep, the inbox, decisions, the host
/// problems from the coordinator, the settings, the key and the navigation. The coordinator starts and stops the
/// sessions; this model makes them, each with a `HostStoreEngine`, and keeps what the engines report.
@Observable @MainActor
final class AppModel {
    private(set) var hosts: [HostConfig]
    /// What keeps a host from connecting, and the keys waiting for Trust, from the coordinator.
    private(set) var problems: [UUID: HostProblem] = [:]
    private(set) var pendingTrust: [UUID: PendingTrust] = [:]
    private(set) var pinsUnreadable = false
    var hostModels: [UUID: HostModel] {
        didSet { modelVersion += 1 }
    }
    var connection: [UUID: ConnectionState]
    var capabilities: [UUID: HostCapabilities]
    var settings: PhoneSettings
    var key: PhoneKeyState
    var busy = BusyTracker()
    var navigation = PhoneNavigation()
    var inboxKind: WaitingKind?
    var inboxSearch = ""
    var agentFilter = AgentFilter.all
    var planFilter = PlanFilter.active
    let overview: OverviewStore

    @ObservationIgnored private(set) var coordinator: HostsCoordinator?
    @ObservationIgnored let runner: (any ProcessRunner)?
    @ObservationIgnored let registry: PhoneHostRegistry?
    @ObservationIgnored let pins: HostKeyStore?
    @ObservationIgnored let vault: (any KeyVault)?
    @ObservationIgnored let directory: URL?
    @ObservationIgnored let pendingStore: PendingStore?
    @ObservationIgnored let decider: Decider?
    @ObservationIgnored var sessions: [UUID: HostSession] = [:]
    @ObservationIgnored var engines: [UUID: HostStoreEngine] = [:]
    @ObservationIgnored var generations: [UUID: Int] = [:]
    @ObservationIgnored var connectionNumber: [UUID: Int] = [:]
    @ObservationIgnored var probes: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored var forwards: [BusyKey: ForwardRecord] = [:]
    @ObservationIgnored var started = false
    /// Counts the changes of `hostModels`; what is derived from the models is kept for one count.
    @ObservationIgnored var modelVersion = 0
    @ObservationIgnored var derived = DerivedReads()
    /// The models as the engines report them; `hostModels` shows them with a brief silent state held back.
    @ObservationIgnored var reportedModels: [UUID: HostModel] = [:]
    @ObservationIgnored var silentDebounce = SilentDebounce()
    @ObservationIgnored var silentTimer: Task<Void, Never>?
    @ObservationIgnored var pendingModels: [UUID: (model: HostModel, generation: Int)] = [:]
    @ObservationIgnored var modelFlushScheduled = false

    nonisolated static let log = Logger(subsystem: "capstan", category: "AppModel")

    // MARK: Creating

    /// The app's model. It makes the coordinator, whose sessions it supplies; nothing runs until `start()`.
    init(
        directory: URL, runner: any ProcessRunner, transport: any TransportControl, registry: PhoneHostRegistry,
        pins: HostKeyStore, vault: any KeyVault
    ) {
        let pending = PendingStore(url: directory.appendingPathComponent("pending.json", isDirectory: false))
        self.directory = directory
        self.runner = runner
        self.registry = registry
        self.pins = pins
        self.vault = vault
        self.pendingStore = pending
        self.decider = Decider(runner: runner, pending: pending, clock: SystemClock())
        self.overview = OverviewStore(runner: runner)
        self.hosts = []
        self.hostModels = [:]
        self.connection = [:]
        self.capabilities = [:]
        self.settings = PhoneSettingsStore(directory: directory).load()
        self.key = AppModel.readKey(vault)
        let made = HostsCoordinator(
            transport: transport, registry: registry, pins: pins, phoneStore: PhoneHostStore(directory: directory),
            hostsFile: HostsFile(directory: directory),
            makeSession: { [weak self] config in
                self?.makeSession(config) ?? IdleSession()
            },
            checker: HostsCoordinator.checker(runner: runner, clock: SystemClock()))
        coordinator = made
        made.onChange = { [weak self] in self?.syncCoordinator() }
    }

    /// A model for previews: nothing is read or written and nothing runs.
    init(preview seed: PreviewSeed) {
        directory = nil
        runner = nil
        registry = nil
        pins = nil
        vault = nil
        pendingStore = nil
        decider = nil
        overview = OverviewStore(runner: nil)
        hosts = seed.hosts
        hostModels = seed.hostModels
        connection = seed.connection
        capabilities = seed.capabilities
        settings = seed.settings
        key = seed.key
        problems = seed.problems
        pendingTrust = seed.pendingTrust
        previewPhoneHosts = Dictionary(uniqueKeysWithValues: seed.phoneHosts.map { ($0.id, $0) })
        previewPins = seed.pins
    }

    @ObservationIgnored private var previewPhoneHosts: [UUID: PhoneHost] = [:]
    @ObservationIgnored private var previewPins: [UUID: HostKeyPin] = [:]

    /// Starts the coordinator: it loads the hosts and starts their sessions. What the engines saved is shown first.
    func start() {
        guard !started, let coordinator else { return }
        started = true
        coordinator.start()
        syncCoordinator()
        loadSaved()
    }

    /// Copies the coordinator's state into this observable model.
    func syncCoordinator() {
        guard let coordinator else { return }
        hosts = coordinator.hosts
        if problems != coordinator.problems {
            for (id, problem) in coordinator.problems where problems[id] != problem {
                let name = coordinator.hosts.first { $0.id == id }?.displayName ?? id.uuidString
                AppModel.log.notice("\(name, privacy: .public): blocked: \(String(describing: problem), privacy: .public)")
            }
        }
        problems = coordinator.problems
        pendingTrust = coordinator.pendingTrust
        pinsUnreadable = coordinator.pinsUnreadable
    }

    private func loadSaved() {
        guard let directory else { return }
        let persistence = FilePersistence(directory: directory)
        for config in hosts {
            guard hostModels[config.id] == nil, let saved = persistence.load(hostId: config.id) else { continue }
            var model = HostModel(hostId: config.id)
            model.projects = saved.projects
            model.cursor = saved.cursor
            model.forwards = saved.forwards
            show(model)
            for (projectId, records) in saved.forwards {
                for record in records {
                    forwards[BusyKey(hostId: config.id, projectId: projectId, requestId: record.requestId)] = record
                }
            }
        }
    }

    /// Where a host is, for display (`hostname:port`); the alias is never shown.
    func phoneHost(_ id: UUID) -> PhoneHost? {
        registry?.host(id) ?? previewPhoneHosts[id]
    }

    /// The key the person trusted for a host, to compare with what the host shows now.
    func trustedPin(_ id: UUID) -> HostKeyPin? {
        pins?.pin(for: id) ?? previewPins[id]
    }

    static func readKey(_ vault: (any KeyVault)?) -> PhoneKeyState {
        guard let vault else { return .none }
        do {
            guard let key = try vault.current() else { return .none }
            let publicKey = key.publicKey
            return .present(
                kind: key.kind, publicLine: OpenSSHKey.publicLine(publicKey, comment: "capstan-iphone"),
                fingerprint: OpenSSHKey.fingerprint(publicKey))
        } catch KeyVaultError.enclaveUnavailable {
            return .unavailable("This device has no Secure Enclave.")
        } catch {
            return .unavailable("The key could not be read.")
        }
    }

    // MARK: Sessions

    /// The coordinator's session factory: an engine and a session for the host, under a new generation.
    private func makeSession(_ config: HostConfig) -> any HostSessionControl {
        guard let directory, let runner else { return IdleSession() }
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
            onNotify: { _ in
                // Notifications arrive with package `scr-settings` and Phase 3; the engine's baseline is kept.
            },
            onRefresh: { [weak self] projectIds in
                Task { @MainActor [weak self] in
                    guard let self, self.generations[id] == generation, let session = self.sessions[id] else { return }
                    await session.refresh(projectIds: projectIds)
                }
            },
            onUrgentRefresh: { [weak self] projectIds in
                Task { @MainActor [weak self] in
                    guard let self, self.generations[id] == generation, let session = self.sessions[id] else { return }
                    await session.refresh(projectIds: projectIds, urgent: true)
                }
            },
            onConnection: { [weak self] state in
                // Numbered here, in the engine's order, because separate tasks may reach the main actor out of order.
                let number = numbering.next()
                Task { @MainActor [weak self] in
                    self?.applyConnection(state, number: number, hostId: id, generation: generation)
                }
            },
            onLiveEvents: { [weak self] projectId, events in
                Task { @MainActor [weak self] in
                    guard let self, self.generations[id] == generation else { return }
                    self.overviewLive(hostId: id, projectId: projectId, events: events)
                }
            })
        var running = config
        running.pollMs = max(config.pollMs, settings.pollMs)
        let hostName = config.displayName
        let session = HostSession(
            config: running, runner: runner, consumer: engine, clock: SystemClock(),
            diagnostic: { what, detail in
                // The detail may be host text (ssh stderr), so it stays private in the log.
                AppModel.log.notice("\(hostName, privacy: .public): \(what, privacy: .public) \(detail ?? "", privacy: .private)")
            })
        engines[id] = engine
        sessions[id] = session
        connection[id] = .connecting
        probe(config, generation: generation)
        return EngineSession(session: session, engine: engine)
    }

    func probe(_ config: HostConfig, generation: Int) {
        guard let runner else { return }
        let id = config.id
        probes[id]?.cancel()
        probes[id] = Task { [weak self] in
            let found = await probeCapabilities(config, runner)
            guard !Task.isCancelled, let self, self.generations[id] == generation else { return }
            self.capabilities[id] = found
        }
    }

    /// Keeps the newest model of each host and applies them together on the next turn of the main actor.
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
            show(entry.model)
        }
    }

    /// Publishes a host's model with each project's state passed through the silent debounce.
    func show(_ model: HostModel, now: Date = Date()) {
        reportedModels[model.hostId] = model
        var shown = model
        for (id, project) in model.projects {
            shown.projects[id]?.availability = silentDebounce.shown(
                SilentDebounce.Key(hostId: model.hostId, projectId: id), raw: project.availability, now: now)
        }
        silentDebounce.retain(Set(reportedModels.flatMap { host, m in
            m.projects.keys.map { SilentDebounce.Key(hostId: host, projectId: $0) }
        }))
        hostModels[model.hostId] = shown
        silentTimer?.cancel()
        silentTimer = nil
        guard let due = silentDebounce.nextDue else { return }
        silentTimer = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(max(0, due.timeIntervalSinceNow)))
            guard !Task.isCancelled, let self else { return }
            for model in self.reportedModels.values where self.hostModels[model.hostId] != nil { self.show(model) }
        }
    }

    private func applyConnection(_ state: ConnectionState, number: Int, hostId: UUID, generation: Int) {
        guard generations[hostId] == generation, number > (connectionNumber[hostId] ?? 0) else { return }
        connectionNumber[hostId] = number
        var reconnected = false
        if case .live = state, case .reconnecting? = connection[hostId] { reconnected = true }
        connection[hostId] = state
        overviewConnection(state, hostId: hostId, engine: engines[hostId])
        if reconnected, let config = hosts.first(where: { $0.id == hostId }) {
            probe(config, generation: generation)
        }
    }

    // MARK: Reading

    /// The groups of every waiting item; kept until the models change.
    private func baseInbox() -> [InboxGroup] {
        let models = hostModels  // read here, so a view that calls this is told of a change even when the answer is kept
        if let kept = derived.base, kept.version == modelVersion { return kept.groups }
        let groups = CapstanKit.inbox(Array(models.values))
        derived.base = (modelVersion, groups)
        return groups
    }

    /// The groups the Waiting tab shows for the kind filter and the search.
    func inbox() -> [InboxGroup] {
        InboxScope(kind: inboxKind, search: inboxSearch).apply(baseInbox())
    }

    /// The same groups as the screen lists them: merged and conflicted integrations apart.
    func displayGroups() -> [DisplayGroup] {
        let key = DerivedReads.GroupsKey(version: modelVersion, kind: inboxKind, search: inboxSearch)
        _ = hostModels
        if let kept = derived.display, kept.key == key { return kept.groups }
        let groups = InboxLayout.groups(inbox())
        derived.display = (key, groups)
        return groups
    }

    /// The number of rows the search leaves, whatever the kind filter: the "All" count.
    func inboxTotal() -> Int {
        InboxScope(kind: nil, search: inboxSearch).apply(baseInbox()).reduce(0) { $0 + $1.count }
    }

    var badgeCount: Int {
        let models = hostModels
        if let kept = derived.badge, kept.version == modelVersion { return kept.count }
        let count = CapstanKit.badgeCount(Array(models.values))
        derived.badge = (modelVersion, count)
        return count
    }

    /// The counts on the tab badges.
    var tabCounts: TabCounts {
        let messages = baseInbox().filter { $0.kind == .message }.reduce(0) { $0 + $1.count }
        return TabCounts(waiting: badgeCount, messages: messages)
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

    /// Whether a verb is offered for a row, or for the host of an agent or run verb.
    func availability(_ verb: DecideVerb, row: InboxRowID?, hostId: UUID? = nil, option: PromptOption?) -> DecideAvailability {
        guard let hostId = row?.hostId ?? hostId, let caps = capabilities[hostId] else {
            return .hidden("The host has not been checked yet")
        }
        let stored = row.flatMap { item($0) }
        return CapstanKit.availability(verb, capabilities: caps, item: stored.flatMap(AppModel.wireItem), option: option)
    }

    static func wireItem(_ stored: StoredWaiting) -> WaitingItem? {
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

    /// The connection capsule of a host (word, tone, pulse); a host problem reads as stopped.
    func capsule(_ hostId: UUID, reduceMotion: Bool) -> CapsuleSpec {
        if problems[hostId] != nil { return CapsuleSpec(label: "Stopped", tone: .red, pulse: false) }
        return CapsuleChoice.make(
            state: connection[hostId], anyLoading: false, heartbeatPulse: settings.heartbeatPulse, reduceMotion: reduceMotion)
    }

    // MARK: Navigation

    /// Opens what a notification, the Live Activity or a widget names. An item that is gone opens its project.
    func open(deepLink link: DeepLink) {
        switch link {
        case .item(let hostId, let projectId, let requestId):
            if let key = project(hostId, projectId)?.waiting.first(where: { $0.value.requestId == requestId })?.key {
                navigation.show(.waiting, [.item(InboxRowID(hostId: hostId, projectId: projectId, key: key))])
            } else {
                navigation.show(.waiting, [.project(hostId: hostId, projectId: projectId)])
            }
        case .agent(let hostId, let projectId, let agentId):
            navigation.show(.agents, [.agent(hostId: hostId, projectId: projectId, agentId: agentId)])
        case .plan(let hostId, let projectId, let planId):
            navigation.show(.plans, [.plan(PlanRef(hostId: hostId, projectId: projectId, planId: planId))])
        case .conversation(let hostId, let projectId):
            navigation.show(.messages, [.conversation(hostId: hostId, projectId: projectId)])
        }
    }

    /// A `capstan://` link the system handed to the app; anything that does not parse is ignored.
    func handle(url: URL) {
        guard let link = DeepLink(url: url) else { return }
        open(deepLink: link)
    }
}

/// A host session and its engine as the coordinator starts and stops them: stopping flushes the engine.
private final class EngineSession: HostSessionControl {
    let session: HostSession
    let engine: HostStoreEngine

    init(session: HostSession, engine: HostStoreEngine) {
        self.session = session
        self.engine = engine
    }

    func start() async { await session.start() }
    func restart() async { await session.restart() }

    func stop() async {
        await session.stop()
        await engine.flush()
    }
}

/// What the coordinator gets when the model is gone or holds no runner (previews).
private final class IdleSession: HostSessionControl {
    func start() async {}
    func stop() async {}
    func restart() async {}
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
struct DerivedReads {
    struct GroupsKey: Equatable {
        let version: Int
        let kind: WaitingKind?
        let search: String
    }

    var base: (version: Int, groups: [InboxGroup])?
    var display: (key: GroupsKey, groups: [DisplayGroup])?
    var badge: (version: Int, count: Int)?
}
