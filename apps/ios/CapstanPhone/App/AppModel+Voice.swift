import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation

/// What the Siri intents run on. Thin: the order of the flow is `VoiceFlow`'s; this only answers its questions with
/// the app's parts. `confirm` and `choose` come from the intent that is running (only an intent can ask aloud).
struct AppVoiceEnvironment: VoiceEnvironment {
    let shell: AppShell
    let ask: @Sendable (String) async -> Bool
    let pick: @Sendable (VoicePrompt) async -> Int?

    func readiness() async -> VoiceReadiness {
        await shell.model.voiceReadiness()
    }

    func settings() async -> PhoneSettings {
        await shell.model.settings
    }

    func nonce() -> String { Nonce.make() }

    func withHosts<T: Sendable>(
        budget: TimeInterval, _ body: @Sendable (VoiceRead) async throws -> T
    ) async throws -> T {
        try await shell.withHosts(budget: budget, body)
    }

    func confirm(_ text: String) async -> Bool { await ask(text) }

    func choose(_ prompt: VoicePrompt) async -> Int? { await pick(prompt) }

    func voiceSend(_ request: DecideRequest) async -> DecideOutcome {
        await shell.model.voiceSend(request)
    }

    var isBackground: Bool { shell.isBackground }
}

extension AppModel {
    /// From the key vault: a throw is a phone not yet unlocked since it started, nil is no key (not paired).
    func voiceReadiness() -> VoiceReadiness {
        do {
            guard try KeychainKeyVault().current() != nil else { return .notPaired }
        } catch {
            return .keysUnreadable
        }
        return hosts.contains(where: \.enabled) ? .ready : .noHosts
    }

    /// Sends a spoken decision to the host it names: one at a time per request, and never again after a failure.
    func voiceSend(_ request: DecideRequest) async -> DecideOutcome {
        let key = BusyKey(hostId: request.hostId, projectId: request.projectId, requestId: request.requestId)
        guard let config = hosts.first(where: { $0.id == request.hostId }), let decider else {
            return LocalOutcome.notWaiting()
        }
        guard busy.begin(key) else { return LocalOutcome.busy() }
        defer { busy.end(key) }
        return await decider.send(request, config: config)
    }
}
