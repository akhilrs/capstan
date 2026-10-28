import Foundation

/// A decision that was sent and has no answer yet. The retry sends the same request, nonce included.
public struct PendingDecision: Sendable, Equatable, Codable, Identifiable {
    public let id: UUID
    public let request: DecideRequest
    public let sentAt: Date

    public init(id: UUID = UUID(), request: DecideRequest, sentAt: Date) {
        self.id = id
        self.request = request
        self.sentAt = sentAt
    }
}

/// Sent, unanswered decisions in one JSON file, written atomically. A missing or corrupt file is an empty store.
public actor PendingStore {
    private let url: URL
    private var records: [PendingDecision]

    public init(url: URL) {
        self.url = url
        self.records = PendingStore.load(url)
    }

    public func all() -> [PendingDecision] {
        records
    }

    public func record(_ id: UUID) -> PendingDecision? {
        records.first { $0.id == id }
    }

    /// Adds a record, or replaces the one with the same id.
    public func add(_ record: PendingDecision) {
        if let index = records.firstIndex(where: { $0.id == record.id }) {
            records[index] = record
        } else {
            records.append(record)
        }
        save()
    }

    public func remove(_ id: UUID) {
        let before = records.count
        records.removeAll { $0.id == id }
        if records.count != before { save() }
    }

    /// Drops every record of a host, for when the host is removed.
    public func remove(hostId: UUID) {
        let before = records.count
        records.removeAll { $0.request.hostId == hostId }
        if records.count != before { save() }
    }

    private static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }

    private static func load(_ url: URL) -> [PendingDecision] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        return (try? decoder().decode([PendingDecision].self, from: data)) ?? []
    }

    /// A failed write keeps the records in memory; the next change tries again.
    private func save() {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        guard let data = try? encoder.encode(records) else { return }
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }
}
