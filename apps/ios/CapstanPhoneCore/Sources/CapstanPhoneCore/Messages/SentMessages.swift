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

    public let id: UUID
    public let hostId: UUID
    public let projectId: String
    public let text: String
    public let sentAt: Date
    public var state: State

    public init(id: UUID = UUID(), hostId: UUID, projectId: String, text: String, sentAt: Date, state: State = .sending) {
        self.id = id
        self.hostId = hostId
        self.projectId = projectId
        self.text = text
        self.sentAt = sentAt
        self.state = state
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
