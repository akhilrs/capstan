import CapstanKit
import CapstanPhoneCore
import CapstanSSH
import SwiftUI
import UIKit

@main
struct CapstanPhoneApp: App {
    @State private var shell = AppShell()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
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
            .onChange(of: scenePhase, initial: true) { _, phase in shell.phaseChanged(phase) }
            .onOpenURL { url in shell.model.handle(url: url) }
            .onReceive(NotificationCenter.default.publisher(for: UIApplication.willTerminateNotification)) { _ in
                shell.shutdown()
            }
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
