import AppIntents
import CapstanPhoneCore

/// Approves with a choice: Siri asks for a number among what the host and the settings allow, reads the choice
/// back, and asks again before an auto-approve window opens.
struct ApproveWithGrantIntent: AppIntent {
    static let title: LocalizedStringResource = "Approve"
    static let openAppWhenRun = false
    static let authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

    @Parameter(title: "Number")
    var number: Int?

    @Dependency
    var shell: AppShell

    @MainActor
    func perform() async throws -> some IntentResult & ProvidesDialog {
        let me = IntentBox(value: self)
        let env = AppVoiceEnvironment(
            shell: shell,
            ask: { text in
                do {
                    try await requestConfirmation(result: .result(dialog: VoiceRun.dialog([text])))
                    return true
                } catch { return false }
            },
            pick: { prompt in
                let spoken = VoiceRun.dialog([prompt.question] + prompt.options)
                guard let said = try? await me.value.$number.requestValue(spoken) else { return nil }
                let index = said - 1
                return prompt.options.indices.contains(index) ? index : nil
            })
        return .result(dialog: VoiceRun.dialog(await VoiceFlow.run(.approve, env)))
    }
}
