import CapstanKit
import CapstanPhoneCore
import BackgroundTasks
import CapstanSSH
import SwiftUI
import UIKit

@main
struct CapstanPhoneApp: App {
    @State private var shell = AppShell()
    @Environment(\.scenePhase) private var scenePhase

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

    /// A background refresh (plan 7.4): start the sessions, wait until every host read is live with a complete
    /// snapshot or the budget is spent, publish the glance (badge, Live Activity, notifications), and stop again if
    /// the app is still in the background. It never asks for Face ID and never decides anything.
    func backgroundRefresh() async {
        if modelStarted {
            model.coordinator?.startSessions()
        } else {
            modelStarted = true
            model.start()
        }
        let started = Date()
        while true {
            let ids = RefreshPlanner.hosts(model.hosts, blocked: Set(model.problems.keys))
            let complete = !ids.isEmpty && ids.allSatisfy { id in
                guard case .live(let since, _)? = model.connection[id], since >= started else { return false }
                return RefreshPlanner.isComplete(model.hostModels[id])
            }
            if RefreshPlanner.shouldStop(started: started, now: Date(), complete: complete) { break }
            try? await Task.sleep(for: .seconds(1))
        }
        // Let the last model reach the main actor, then publish what was read.
        try? await Task.sleep(for: .milliseconds(500))
        model.publishGlance()
        BackgroundRefresh.schedule()
        if UIApplication.shared.applicationState == .background {
            model.coordinator?.stopSessionsThenSuspend()
        }
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
