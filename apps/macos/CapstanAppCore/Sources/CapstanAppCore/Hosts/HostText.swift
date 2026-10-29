import CapstanKit
import Foundation

/// Host-supplied strings, cleaned and capped before they are placed in a text.
enum HostText {
    static let nameCap = 64
    static let codeCap = 256
    static let detailCap = 4096

    /// Control, bidi and line characters become spaces; then the text is cut to `maxBytes` between characters.
    static func clean(_ s: String, maxBytes: Int) -> String {
        let plain = UntrustedText(raw: s, verified: true, truncated: false).displayPlain
        return TextBudget.displayPrefix(plain, maxBytes: maxBytes).text
    }

    static func name(_ s: String) -> String { clean(s, maxBytes: nameCap) }
    static func code(_ s: String) -> String { clean(s, maxBytes: codeCap) }

    /// True when a host's stderr says its `cstan` has no `events` command, which is what an old cstan prints.
    static func mentionsUnknownEventsCommand(_ stderr: String) -> Bool {
        stderr.contains("unknown command: events")
    }

    /// A host text for a detail block: nil when it is not verified or empty after cleaning.
    static func detail(_ t: UntrustedText) -> UntrustedText? {
        guard t.verified else { return nil }
        let plain = t.displayPlain
        guard !plain.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        let cut = TextBudget.displayPrefix(plain, maxBytes: detailCap)
        return UntrustedText(raw: cut.text, verified: true, truncated: cut.cut || t.truncated)
    }

    /// Appended when ssh failed on the host key or the login, which a terminal run can settle.
    static func sshHint(_ text: String, destination: String) -> String? {
        guard text.contains("Host key verification failed") || text.contains("Permission denied") else {
            return nil
        }
        return "Run ssh \(name(destination)) once in a terminal."
    }
}

public enum ProjectName {
    /// The last path component; "/" stays "/".
    public static func display(root: String) -> String {
        if let last = root.split(separator: "/", omittingEmptySubsequences: true).last { return String(last) }
        return root.isEmpty ? "/" : root
    }
}

public struct AvailabilityText: Sendable, Equatable {
    public let word: String
    public let symbol: String
    public let meaning: String
    public let fix: String

    public init(_ availability: ProjectAvailability) {
        switch availability {
        case .ready:
            self.init("Ready", "checkmark.circle", "The project is sending events.", "Nothing to do.")
        case .noKey:
            self.init(
                "No key", "key.slash", "The project has no events key on the host.",
                "Run cstan on the host to set up the project, then refresh.")
        case .silent:
            self.init(
                "Silent", "speaker.slash", "The project has not sent events lately.",
                "Check that the project's Capstan is running on the host.")
        case .refused:
            self.init(
                "Refused", "xmark.octagon", "The host refused to share this project's events.",
                "Check the project's settings on the host.")
        case .unsupported:
            self.init(
                "Unsupported", "exclamationmark.triangle", "The project's Capstan is too old for this app.",
                "Update Capstan on the host.")
        case .gone:
            self.init(
                "Gone", "questionmark.folder", "The project folder is no longer on the host.",
                "Restore the folder or remove the project.")
        case .other(let wire):
            self.init(
                "Unknown", "questionmark.circle",
                "The host reported a state this app does not know: \(HostText.name(wire)).",
                "Update this app.")
        }
    }

    private init(_ word: String, _ symbol: String, _ meaning: String, _ fix: String) {
        self.word = word
        self.symbol = symbol
        self.meaning = meaning
        self.fix = fix
    }
}

/// A project of the host has no complete snapshot yet: a first load, or a reload after a resync.
/// True while a ready project still waits for its snapshot. A project that is not ready (unsupported, refused, gone)
/// never gets one, so it must not keep the host in "Loading".
public func anyLoading(_ host: HostModel?) -> Bool {
    host?.projects.values.contains { $0.availability == .ready && !$0.snapshotComplete } ?? false
}

/// The one state that stands for several hosts: live if any host is live, then the next most connected.
public func aggregateConnection(_ states: [ConnectionState]) -> ConnectionState? {
    func rank(_ state: ConnectionState) -> Int {
        switch state {
        case .live: return 0
        case .noHeartbeat: return 1
        case .reconnecting: return 2
        case .connecting: return 3
        case .stopped(.user): return 5
        case .stopped: return 4
        }
    }
    return states.min { rank($0) < rank($1) }
}

public enum HostLimits {
    /// Copies the limit of `cstan events --poll-ms`.
    public static let maxPollMs = 3_600_000

    public static func pollError(_ config: HostConfig) -> String? {
        if config.validate().contains(.pollMs) { return HostConfigError.pollMs.message }
        if config.pollMs > maxPollMs { return "At most \(maxPollMs) ms" }
        return nil
    }
}
