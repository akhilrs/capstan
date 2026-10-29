import Foundation

/// A decision that the host forwarded to the terminal that owns the item. It is shown while the item keeps the
/// content hash it had when the decision was forwarded.
public struct ForwardRecord: Codable, Sendable, Equatable {
    public var requestId: String
    public var contentHash: String
    public var messageId: String?
    public var at: Date

    public init(requestId: String, contentHash: String, messageId: String? = nil, at: Date) {
        self.requestId = requestId
        self.contentHash = contentHash
        self.messageId = messageId
        self.at = at
    }

    /// True while the item is still the one that was forwarded.
    public func covers(_ item: StoredWaiting) -> Bool {
        item.requestId == requestId && item.contentHash == contentHash
    }
}
