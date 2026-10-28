import Foundation

/// The caps of the remote protocol (document sections 8 and 10).
public enum ProtocolLimits {
    /// Event text, in bytes.
    public static let textCap = 4096
    /// A reason or note, in bytes.
    public static let reasonCap = 1024
    /// Options of one prompt.
    public static let optionsCap = 32
    /// One option, in bytes.
    public static let optionTextCap = 256
    /// A snapshot item's summary, task or title, in bytes.
    public static let snapshotTextCap = 1024
    /// Events per page.
    public static let eventsPerPage = 500
    /// Snapshot items per page.
    public static let itemsPerPage = 100
    /// Row reads per events page.
    public static let eventRowReads = 2000
    /// Row reads per items page.
    public static let itemRowReads = 2000
    /// Serialized bytes of one page.
    public static let pageBytes = 524_288
    /// Bytes of one event.
    public static let eventBytes = 49_152
    /// Bytes of one stream line, the newline included.
    public static let lineBytes = 65_536
    /// The smallest poll interval, in milliseconds.
    public static let minPollMs = 1000
    /// Projects one stream follows.
    public static let maxProjects = 64
}
