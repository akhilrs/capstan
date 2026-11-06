import AppIntents
import CapstanKit
import CapstanPhoneCore
import BackgroundTasks
import CapstanSSH
import SwiftUI
import UIKit
import os

@main
struct CapstanPhoneApp: App {
    @State private var shell: AppShell
    @Environment(\.scenePhase) private var scenePhase

    /// The shell is made and registered here, before any scene: Siri may start the app cold with no window.
    init() {
        let made = AppShell()
        _shell = State(initialValue: made)
        AppDependencyManager.shared.add(dependency: made)
    }

    var body: some Scene {
        let shell = self.shell
        return WindowGroup {
            ZStack {
                RootTabView()
                    .environment(shell.model)
                    .opacity(shell.locked ? 0 : 1)
                    .accessibilityHidden(shell.locked)
                    .allowsHitTesting(!shell.locked)
                if shell.locked {
                    LockOverlay { shell.promptFaceID() }
                }
            }
            .onboarding(shell.model, enabled: !shell.locked)
            .onChange(of: scenePhase, initial: true) { _, phase in
                shell.phaseChanged(phase)
                if phase == .background { BackgroundRefresh.schedule() }
            }
            .onOpenURL { url in shell.model.handle(url: url) }
            .onReceive(NotificationCenter.default.publisher(for: UIApplication.willTerminateNotification)) { _ in
                shell.shutdown()
            }
        }
        .backgroundTask(.appRefresh(BackgroundRefresh.identifier)) {
            await shell.backgroundRefresh()
        }
    }
}

/// The app's process-wide parts: the one `SSHRunner`, the `AppModel` (which owns the hosts coordinator) and the Face ID
/// lock, driven by `LifecyclePlanner`.
@MainActor
@Observable
final class AppShell {
    let runner: SSHRunner
    let model: AppModel
    /// The content is covered until Face ID passes.
    private(set) var locked: Bool

    private var lifecycle: LifecycleState
    private var modelStarted = false
    /// The Face ID sheet itself makes the scene inactive and active again; one prompt at a time.
    private var prompting = false
    private let gate: any BiometricGate = FaceIDGate()
    /// The voice runs that hold the sessions open (design: the first starts them, the last stops them in the
    /// background).
    private let lease = SessionLeaseBox()
    /// Whether the app is in the background; true until a scene says otherwise (a cold start from Siri has none).
    private let backgroundFlag = OSAllocatedUnfairLock(initialState: true)
    nonisolated var isBackground: Bool { backgroundFlag.withLock { $0 } }

    init() {
        let directory = AppShell.supportDirectory()
        let pins = HostKeyStore(directory: directory)
        let registry = PhoneHostRegistry(store: pins)
        let vault = KeychainKeyVault()
        let runner = SSHRunner(
            registry: registry, vault: vault, trust: pins, tuning: TransportTuning(), clock: SystemClock(),
            hook: TuningHook())
        // Read into a local: @Observable storage may not be read before every property is set.
        let made = AppModel(
            directory: directory, runner: runner, transport: runner, registry: registry, pins: pins, vault: vault)
        self.runner = runner
        model = made
        locked = made.settings.faceID
        lifecycle = LifecycleState(locked: made.settings.faceID)
        made.notifier.onOpen = { [weak made] url in made?.handle(url: url) }
    }

    /// Starts the sessions without the lifecycle: no lock, no Face ID prompt and no notification request.
    private func startSessionsQuietly() {
        if modelStarted {
            model.coordinator?.startSessions()
        } else {
            modelStarted = true
            model.start()
        }
    }

    /// Runs `body` with the host sessions held open (a `SessionLease`): the first holder starts them, waits until
    /// every host read is live with a complete snapshot or the budget is spent, and the last holder stops them
    /// again while the app is in the background. It never asks for Face ID.
    func withHosts<T: Sendable>(
        budget: TimeInterval, _ body: @Sendable (VoiceRead) async throws -> T
    ) async throws -> T {
        try await lease.hold(
            isBackground: { self.isBackground },
            start: { await MainActor.run { self.startSessionsQuietly() } },
            stop: { await MainActor.run { self.model.coordinator?.stopSessionsThenSuspend() } },
            body: {
                let read = await self.readHosts(budget: budget)
                return try await body(read)
            })
    }

    private func readHosts(budget: TimeInterval) async -> VoiceRead {
        let started = Date()
        // A foreground app keeps its sessions, so what is live already counts; after a start, only a new connection.
        let fresh = isBackground
        while true {
            if hostsComplete(since: fresh ? started : nil) || Date().timeIntervalSince(started) >= budget { break }
            try? await Task.sleep(for: .milliseconds(250))
        }
        // Let the last model reach the main actor.
        try? await Task.sleep(for: .milliseconds(250))
        let ids = RefreshPlanner.hosts(model.hosts, blocked: Set(model.problems.keys))
        var models: [VoiceHostModel] = []
        var down: [VoiceHostRef] = []
        for config in model.hosts where ids.contains(config.id) {
            if let hostModel = model.hostModels[config.id], RefreshPlanner.isComplete(hostModel),
                isLive(config.id, since: fresh ? started : nil)
            {
                models.append(VoiceHostModel(id: config.id, name: config.displayName, model: hostModel))
            } else {
                down.append(VoiceHostRef(id: config.id, name: config.displayName))
            }
        }
        return VoiceRead(models: models, unreachable: down, now: Date())
    }

    private func isLive(_ id: UUID, since: Date?) -> Bool {
        guard case .live(let at, _)? = model.connection[id] else { return false }
        return since.map { at >= $0 } ?? true
    }

    private func hostsComplete(since: Date?) -> Bool {
        let ids = RefreshPlanner.hosts(model.hosts, blocked: Set(model.problems.keys))
        return !ids.isEmpty && ids.allSatisfy { isLive($0, since: since) && RefreshPlanner.isComplete(model.hostModels[$0]) }
    }

    /// A background refresh (plan 7.4): read every host within the old budget, publish the glance (badge, Live
    /// Activity, notifications), and schedule the next run. It never asks for Face ID and never decides anything.
    func backgroundRefresh() async {
        _ = try? await withHosts(budget: RefreshPlanner.budgetSeconds) { _ in }
        model.publishGlance()
        BackgroundRefresh.schedule()
    }

    private static func supportDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        let url = base.appendingPathComponent("Capstan", isDirectory: true)
        try? FileManager.default.createDirectory(
            at: url, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        return url
    }

    func phaseChanged(_ phase: ScenePhase) {
        backgroundFlag.withLock { $0 = phase == .background }
        let mapped: PhonePhase
        switch phase {
        case .active: mapped = .active
        case .inactive: mapped = .inactive
        case .background: mapped = .background
        @unknown default: return
        }
        apply(LifecyclePlanner.phase(mapped, state: lifecycle, faceIDOn: model.settings.faceID, now: Date()))
    }

    func promptFaceID() {
        guard !prompting else { return }
        prompting = true
        Task {
            let passed = await gate.unlock(reason: "Unlock Capstan to see what your agents wait for.")
            prompting = false
            apply(passed
                ? LifecyclePlanner.unlocked(state: lifecycle, now: Date())
                : LifecyclePlanner.unlockFailed(state: lifecycle))
        }
    }

    func shutdown() {
        runner.shutdown()
    }

    private func apply(_ plan: LifecyclePlanner.Plan) {
        // What the lifecycle decided, for the device checks (no host text: phases, times and actions only).
        let away = plan.state.backgroundedAt.map { String(Int(Date().timeIntervalSince($0))) + " s" } ?? "-"
        AppModel.log.notice(
            "lifecycle: \(String(describing: plan.actions), privacy: .public) locked \(plan.state.locked, privacy: .public) unlockedBefore \(plan.state.lastUnlock != nil, privacy: .public) away \(away, privacy: .public)")
        lifecycle = plan.state
        for action in plan.actions {
            switch action {
            case .lockUI:
                locked = true
            case .unlockUI:
                locked = false
            case .promptFaceID:
                promptFaceID()
            case .startSessions:
                if modelStarted {
                    model.coordinator?.startSessions()
                } else {
                    modelStarted = true
                    model.start()
                }
                model.notifier.requestAuthorization()
            case .stopSessionsThenSuspend:
                model.coordinator?.stopSessionsThenSuspend()
            case .noop:
                break
            }
        }
    }
}

/// Opaque and full screen: nothing of the content shows until Face ID passes.
struct LockOverlay: View {
    let unlock: () -> Void

    var body: some View {
        ZStack {
            Tokens.color(.ground).ignoresSafeArea()
            VStack(spacing: Tokens.space[4]) {
                Image(systemName: "lock.fill")
                    .font(.system(size: 44))
                    .foregroundStyle(Tokens.color(.label2))
                    .accessibilityHidden(true)
                Text("Capstan is locked").font(TypeStyle.title2)
                Text("Unlock to see what your agents wait for.")
                    .font(TypeStyle.body)
                    .foregroundStyle(Tokens.color(.label2))
                    .multilineTextAlignment(.center)
                Button("Unlock with Face ID") { unlock() }
                    .buttonStyle(.capstanPrimary)
            }
            .padding(Tokens.margin)
        }
        .accessibilityAddTraits(.isModal)
    }
}

#Preview("LockOverlay") {
    LockOverlay {}
}

/// The background refresh task: its identifier (listed in Info.plist) and the request for the next run. iOS decides
/// when it runs; "less often than the Mac" is expected (R5).
enum BackgroundRefresh {
    static let identifier = "in.qusol.capstan.phone.refresh"
    /// The earliest a refresh is asked for after the last one.
    static let interval: TimeInterval = 15 * 60

    static func schedule() {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: interval)
        try? BGTaskScheduler.shared.submit(request)
    }
}
