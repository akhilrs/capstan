import AppIntents
import CapstanPhoneCore

/// Reads what waits. It decides nothing.
struct WhatsWaitingIntent: AppIntent {
    static let title: LocalizedStringResource = "What's waiting"
    static let openAppWhenRun = false
    static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

    @Dependency
    var shell: AppShell

    @MainActor
    func perform() async throws -> some IntentResult & ProvidesDialog {
        let env = AppVoiceEnvironment(shell: shell, ask: { _ in false }, pick: { _ in nil })
        return .result(dialog: VoiceRun.dialog(await VoiceFlow.run(.list, env)))
    }
}
