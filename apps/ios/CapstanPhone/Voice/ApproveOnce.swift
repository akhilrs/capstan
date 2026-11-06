import AppIntents
import CapstanPhoneCore

/// Approves the one Operator command that waits, once, after the read-back and a yes.
struct ApproveOnceIntent: AppIntent {
    static let title: LocalizedStringResource = "Approve once"
    static let openAppWhenRun = false
    static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

    @Dependency
    var shell: AppShell

    @MainActor
    func perform() async throws -> some IntentResult & ProvidesDialog {
        let env = AppVoiceEnvironment(
            shell: shell,
            ask: { text in
                do {
                    try await requestConfirmation(result: .result(dialog: VoiceRun.dialog([text])))
                    return true
                } catch { return false }
            },
            pick: { _ in nil })
        return .result(dialog: VoiceRun.dialog(await VoiceFlow.run(.approveOnce, env)))
    }
}
