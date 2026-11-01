import CapstanKit
import CapstanPhoneCore
import Foundation
import Observation

/// The messages this phone sent to the PMs, observable for the Messages tab and kept in `messages.json`.
@Observable @MainActor
final class MessagesStore {
    private(set) var log: SentMessageLog
    @ObservationIgnored private let store: SentMessageStore?

    /// nil keeps the messages in memory only (previews).
    init(directory: URL?, seed: SentMessageLog = SentMessageLog()) {
        store = directory.map(SentMessageStore.init(directory:))
        log = store?.load() ?? seed
    }

    func thread(hostId: UUID, projectId: String) -> [SentMessage] {
        log.thread(hostId: hostId, projectId: projectId)
    }

    func append(_ message: SentMessage) {
        log.append(message)
        save()
    }

    func update(_ id: UUID, state: SentMessage.State) {
        log.update(id, state: state)
        save()
    }

    /// Applies the host's message events to the messages this phone sent (Queued, Delivered, Read).
    func noteEvents(_ events: [FeedEvent]) {
        var changed = false
        for event in events where event.type == "message.action_needed" || event.type == "message.state" {
            func text(_ key: String) -> String? {
                if case .string(let value)? = event.body.raw[key] { return value }
                return nil
            }
            guard let id = text("messageId"), let state = text("state") else { continue }
            let before = log
            let from = text("requeuedFrom")
            log.noteDelivery(messageId: id, hostState: state, at: DetailTime.parse(event.ts) ?? Date(), requeuedFrom: from)
            changed = changed || log != before
        }
        if changed { save() }
    }

    func removeHost(_ hostId: UUID) {
        log.removeHost(hostId)
        save()
    }

    private func save() {
        try? store?.save(log)
    }
}
