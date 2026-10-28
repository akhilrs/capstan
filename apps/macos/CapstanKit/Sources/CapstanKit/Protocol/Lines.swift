import Foundation

public enum LineDecodeError: Error, Equatable, Sendable {
    case unsupportedVersion(Int)
    case malformed(String)
    case tooLong
}

public enum ProjectAvailability: Sendable, Equatable {
    case ready, noKey, silent, refused, unsupported, gone
    case other(String)

    /// The wire names of the known states.
    public static let wireNames = ["ready", "no_key", "silent", "refused", "unsupported", "gone"]

    public init(wire: String) {
        switch wire {
        case "ready": self = .ready
        case "no_key": self = .noKey
        case "silent": self = .silent
        case "refused": self = .refused
        case "unsupported": self = .unsupported
        case "gone": self = .gone
        default: self = .other(wire)
        }
    }
}

public enum ResyncReason: Sendable, Equatable {
    case epoch, anchor, ahead, effect, cursor
    case other(String)

    public static let wireNames = ["epoch", "anchor", "ahead", "effect", "cursor"]

    public init(wire: String) {
        switch wire {
        case "epoch": self = .epoch
        case "anchor": self = .anchor
        case "ahead": self = .ahead
        case "effect": self = .effect
        case "cursor": self = .cursor
        default: self = .other(wire)
        }
    }
}

public struct Hello: Sendable, Equatable {
    public let version: String
    public let host: String
    public let projects: Int
}

public struct ProjectLine: Sendable, Equatable {
    public let projectId: String
    public let root: String
    public let state: ProjectAvailability
    public let epoch: String?
    public let head: Int64?
}

public struct SnapshotLine: Sendable, Equatable {
    public let projectId: String
    public let section: String
    public let key: String
    public let item: SnapshotItem
}

public enum StreamLine: Sendable, Equatable {
    case hello(Hello)
    case project(ProjectLine)
    case snapshot(SnapshotLine)
    case snapshotEnd(projectId: String, head: Int64)
    case event(projectId: String, FeedEvent)
    case cursor(Cursor)
    case heartbeat(ts: String)
    case resync(projectId: String?, reason: ResyncReason)
    case error(code: String, message: UntrustedText)
    case unknownKind(String)

    /// The line kinds named in document section 2.
    public static let wireKinds = [
        "hello", "project", "snapshot", "snapshotEnd", "event", "cursor", "heartbeat", "resync", "error",
    ]
}

public struct LineDecoder: Sendable {
    /// Bytes of one stream line, the newline included (document section 1).
    public static let maxLineBytes = 65536

    public init() {}

    /// Decodes one line, given without its newline.
    public func decode(_ line: Data) throws -> StreamLine {
        if line.count + 1 > LineDecoder.maxLineBytes { throw LineDecodeError.tooLong }
        let root = try decodeJSON(line)
        _ = try root.members("line")
        guard case .int(let version)? = root["v"] else { throw LineDecodeError.malformed("missing v") }
        guard version == 1 else { throw LineDecodeError.unsupportedVersion(Int(clamping: version)) }
        let kind = try root.string("kind")
        switch kind {
        case "hello":
            let projects = try root.int64("projects")
            guard let count = Int(exactly: projects) else { throw LineDecodeError.malformed("projects out of range") }
            return .hello(Hello(version: try root.string("version"), host: try root.string("host"), projects: count))
        case "project":
            return .project(ProjectLine(
                projectId: try root.string("projectId"), root: try root.string("root"),
                state: ProjectAvailability(wire: try root.string("state")),
                epoch: try root.optionalString("epoch"), head: try root.optionalInt64("head")))
        case "snapshot":
            let section = try root.string("section")
            return .snapshot(SnapshotLine(
                projectId: try root.string("projectId"), section: section, key: try root.string("key"),
                item: try SnapshotItem(section: section, json: try root.required("item"))))
        case "snapshotEnd":
            return .snapshotEnd(projectId: try root.string("projectId"), head: try root.int64("head"))
        case "event":
            return .event(projectId: try root.string("projectId"), try FeedEvent(json: try root.required("event")))
        case "cursor":
            guard let cursor = Cursor(text: try root.string("cursor")) else {
                throw LineDecodeError.malformed("cursor text")
            }
            return .cursor(cursor)
        case "heartbeat":
            return .heartbeat(ts: try root.string("ts"))
        case "resync":
            return .resync(
                projectId: try root.optionalString("projectId"),
                reason: ResyncReason(wire: try root.string("reason")))
        case "error":
            return .error(
                code: try root.string("code"),
                message: UntrustedText(raw: try root.string("message"), verified: true, truncated: false))
        default:
            return .unknownKind(kind)
        }
    }
}
