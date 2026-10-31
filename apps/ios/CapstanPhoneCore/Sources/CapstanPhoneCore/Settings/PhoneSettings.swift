import CapstanAppCore
import CapstanKit
import Foundation

/// The iPhone's settings (board 14), kept as versioned JSON in Application Support.
public struct PhoneSettings: Codable, Equatable, Sendable {
    public static let currentVersion = 1

    public var version: Int
    /// Face ID once per activation and after a minute away (`FaceIDRule`); on by default.
    public var faceID: Bool
    /// The poll interval of every host's events stream; at least 1000 ms.
    public var pollMs: Int
    public var heartbeatPulse: Bool
    /// The kinds that make a notification.
    public var notifyKinds: Set<WaitingKind>
    /// A sound per kind, as on the Mac.
    public var sounds: SoundPreferences
    /// The kinds whose notifications are Time Sensitive; none until the person switches one on.
    public var timeSensitiveKinds: Set<WaitingKind>
    /// Off until the person allows the Live Activity.
    public var liveActivityAllowed: Bool
    /// The `--client` label of decisions sent from this phone.
    public var clientLabel: String

    public init(
        faceID: Bool = true, pollMs: Int = 2000, heartbeatPulse: Bool = true,
        notifyKinds: Set<WaitingKind> = [.op, .prompt, .design, .message, .integration],
        sounds: SoundPreferences = SoundPreferences(), timeSensitiveKinds: Set<WaitingKind> = [],
        liveActivityAllowed: Bool = false, clientLabel: String = "iPhone"
    ) {
        version = PhoneSettings.currentVersion
        self.faceID = faceID
        self.pollMs = pollMs
        self.heartbeatPulse = heartbeatPulse
        self.notifyKinds = notifyKinds
        self.sounds = sounds
        self.timeSensitiveKinds = timeSensitiveKinds
        self.liveActivityAllowed = liveActivityAllowed
        self.clientLabel = clientLabel
    }

    /// A key that is missing takes its default; keys this version does not know are ignored.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let base = PhoneSettings()
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? base.version
        faceID = try c.decodeIfPresent(Bool.self, forKey: .faceID) ?? base.faceID
        pollMs = try c.decodeIfPresent(Int.self, forKey: .pollMs) ?? base.pollMs
        heartbeatPulse = try c.decodeIfPresent(Bool.self, forKey: .heartbeatPulse) ?? base.heartbeatPulse
        notifyKinds = try c.decodeIfPresent(Set<WaitingKind>.self, forKey: .notifyKinds) ?? base.notifyKinds
        sounds = try c.decodeIfPresent(SoundPreferences.self, forKey: .sounds) ?? base.sounds
        timeSensitiveKinds = try c.decodeIfPresent(Set<WaitingKind>.self, forKey: .timeSensitiveKinds)
            ?? base.timeSensitiveKinds
        liveActivityAllowed = try c.decodeIfPresent(Bool.self, forKey: .liveActivityAllowed) ?? base.liveActivityAllowed
        clientLabel = try c.decodeIfPresent(String.self, forKey: .clientLabel) ?? base.clientLabel
    }
}

/// `settings.json` beside the host files.
public struct PhoneSettingsStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var file: URL {
        directory.appendingPathComponent("settings.json", isDirectory: false)
    }

    /// A missing or damaged file gives the defaults.
    public func load() -> PhoneSettings {
        guard let data = try? Data(contentsOf: file),
              let settings = try? JSONDecoder().decode(PhoneSettings.self, from: data) else { return PhoneSettings() }
        return settings
    }

    public func save(_ settings: PhoneSettings) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(settings).write(to: file, options: .atomic)
    }
}

/// The checks of the Settings form, the same as the Mac's host form.
public enum SettingsValidation {
    public static let pollStepMs = 500

    /// The inline error for a poll interval, or nil when it is valid.
    public static func pollError(_ pollMs: Int) -> String? {
        if pollMs < ProtocolLimits.minPollMs { return HostConfigError.pollMs.message }
        if pollMs > HostLimits.maxPollMs { return "At most \(HostLimits.maxPollMs) ms" }
        return nil
    }

    /// The stepper's next value: one step up or down, kept within the limits.
    public static func step(_ pollMs: Int, up: Bool) -> Int {
        clampPoll(pollMs + (up ? pollStepMs : -pollStepMs))
    }

    public static func clampPoll(_ pollMs: Int) -> Int {
        min(max(pollMs, ProtocolLimits.minPollMs), HostLimits.maxPollMs)
    }

    /// The client label rule of `cstan decide --client`: 1 to 64 bytes of letters, digits, space . _ -.
    public static func isClientLabel(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard (1...64).contains(bytes.count) else { return false }
        return bytes.allSatisfy { byte in
            (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || byte == 0x20 || byte == 0x2E || byte == 0x5F || byte == 0x2D
        }
    }
}
