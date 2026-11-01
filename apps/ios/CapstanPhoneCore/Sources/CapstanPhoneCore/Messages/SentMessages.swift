import Foundation

/// A message this phone sent to a project's PM (`pm message`), as the Messages tab shows it. The text is the person's
/// own; what the PM says comes from the host and is never kept here.
public struct SentMessage: Codable, Sendable, Equatable, Identifiable {
    public enum State: Codable, Sendable, Equatable {
        case sending
        /// The host took it; the PM's inbox has it as `messageId`.
        case sent(messageId: String)
        /// No answer came, or the host refused it; `pendingId` retries a request that got no answer.
        case notSent(reason: String, pendingId: UUID?)
    }

    /// Where a message the host took is on its way to the PM, from the host's message events.
    public enum Delivery: Codable, Sendable, Equatable {
        /// Waiting in the PM's queue: the PM is busy, stuck at a question, or paused.
        case queued(since: Date)
        /// Typed into the PM's terminal.
        case delivered(at: Date)
        /// The PM acknowledged it.
        case read(at: Date)
        /// The host gave up on it (cancelled, expired or failed); `state` is the host's word.
        case dropped(state: String, at: Date)
    }

    public let id: UUID
    public let hostId: UUID
    public let projectId: String
    public let text: String
    public let sentAt: Date
    public var state: State
    /// Nil until the host says; saved files from before it decode it as nil.
    public var delivery: Delivery?

    public init(
        id: UUID = UUID(), hostId: UUID, projectId: String, text: String, sentAt: Date, state: State = .sending,
        delivery: Delivery? = nil
    ) {
        self.id = id
        self.hostId = hostId
        self.projectId = projectId
        self.text = text
        self.sentAt = sentAt
        self.state = state
        self.delivery = delivery
    }

    /// The host's message id, once it took the message.
    public var messageId: String? {
        if case .sent(let id) = state { return id }
        return nil
    }

    /// Queued for longer than `seconds` at `now`: the PM is probably stuck.
    public func stuck(now: Date, after seconds: TimeInterval = 180) -> Bool {
        if case .queued(let since)? = delivery { return now.timeIntervalSince(since) > seconds }
        return false
    }
}

/// The sent messages of every project, oldest first, kept in `messages.json`. A project keeps its last `perProjectCap`.
public struct SentMessageLog: Codable, Sendable, Equatable {
    public static let perProjectCap = 200
    /// The longest message, in UTF-8 bytes (board 11's counter).
    public static let textCap = 500

    public private(set) var messages: [SentMessage] = []

    public init(messages: [SentMessage] = []) {
        self.messages = messages
    }

    public func thread(hostId: UUID, projectId: String) -> [SentMessage] {
        messages.filter { $0.hostId == hostId && $0.projectId == projectId }
    }

    public mutating func append(_ message: SentMessage) {
        messages.append(message)
        let mine = thread(hostId: message.hostId, projectId: message.projectId)
        if mine.count > SentMessageLog.perProjectCap {
            let drop = Set(mine.prefix(mine.count - SentMessageLog.perProjectCap).map(\.id))
            messages.removeAll { drop.contains($0.id) }
        }
    }

    public mutating func update(_ id: UUID, state: SentMessage.State) {
        guard let index = messages.firstIndex(where: { $0.id == id }) else { return }
        messages[index].state = state
    }

    /// Applies a host message event to the message it names. `requeuedFrom` is the id of the message a PM restart queued
    /// again as `messageId` for the new PM: the sent message follows it to its new id. Unknown ids change nothing;
    /// an older state never replaces a newer one.
    public mutating func noteDelivery(messageId: String, hostState: String, at: Date, requeuedFrom: String? = nil) {
        let target = requeuedFrom ?? messageId
        guard let index = messages.firstIndex(where: { $0.messageId == target }) else { return }
        if let requeuedFrom, requeuedFrom != messageId {
            messages[index].state = .sent(messageId: messageId)
        }
        let next: SentMessage.Delivery
        switch hostState {
        case "queued", "deferred": next = .queued(since: at)
        case "sent", "unacked": next = .delivered(at: at)
        case "acked", "acked_late": next = .read(at: at)
        default: next = .dropped(state: hostState, at: at)
        }
        if let current = messages[index].delivery, SentMessageLog.rank(current) > SentMessageLog.rank(next),
           requeuedFrom == nil {
            return
        }
        // A queued state keeps the time it first queued.
        if case .queued = next, case .queued? = messages[index].delivery { return }
        messages[index].delivery = next
    }

    static func rank(_ delivery: SentMessage.Delivery) -> Int {
        switch delivery {
        case .queued: return 0
        case .delivered: return 1
        case .read: return 2
        case .dropped: return 1
        }
    }

    /// A host that was removed takes its messages with it.
    public mutating func removeHost(_ hostId: UUID) {
        messages.removeAll { $0.hostId == hostId }
    }

    /// A message still `sending` when the app stopped never got its answer: it reads as not sent.
    public mutating func settleInterrupted() {
        for index in messages.indices where messages[index].state == .sending {
            messages[index].state = .notSent(reason: "The app stopped before the host answered", pendingId: nil)
        }
    }
}

/// `messages.json` beside the host files.
public struct SentMessageStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var file: URL { directory.appendingPathComponent("messages.json", isDirectory: false) }

    public func load() -> SentMessageLog {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        guard let data = try? Data(contentsOf: file), var log = try? decoder.decode(SentMessageLog.self, from: data) else {
            return SentMessageLog()
        }
        log.settleInterrupted()
        return log
    }

    public func save(_ log: SentMessageLog) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        try encoder.encode(log).write(to: file, options: .atomic)
    }
}
