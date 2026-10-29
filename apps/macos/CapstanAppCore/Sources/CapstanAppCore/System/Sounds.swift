import CapstanKit
import Foundation

/// One sound the person can pick: nothing, a system alert sound or a sound from their own folder. A file is named
/// by its file name, with the extension.
public enum SoundChoice: Codable, Equatable, Hashable, Sendable {
    case silent
    case system(String)
    case user(String)

    /// The file name of a sound that has one.
    public var fileName: String? {
        switch self {
        case .silent: return nil
        case .system(let name), .user(let name): return name
        }
    }
}

/// Where a catalog entry comes from.
public enum SoundGroup: Int, Equatable, Hashable, Sendable {
    case silent, system, user
}

public struct SoundEntry: Equatable, Hashable, Sendable, Identifiable {
    public let choice: SoundChoice
    public let group: SoundGroup
    /// The name shown: the file name without its extension; "None" for no sound.
    public let title: String

    public var id: SoundChoice { choice }
}

/// The sounds to offer: None, then the system alert sounds, then the person's own, each sorted by name.
public struct SoundCatalog: Equatable, Sendable {
    public static let extensions: Set<String> = ["aiff", "wav", "caf"]

    public let entries: [SoundEntry]

    /// `systemFiles` and `userFiles` are file names as a directory listing gives them. Names that are hidden, hold a
    /// path separator or have another extension are left out.
    public init(systemFiles: [String], userFiles: [String]) {
        var all = [SoundEntry(choice: .silent, group: .silent, title: "None")]
        all += SoundCatalog.entries(systemFiles, group: .system) { .system($0) }
        all += SoundCatalog.entries(userFiles, group: .user) { .user($0) }
        entries = all
    }

    public var system: [SoundEntry] { entries.filter { $0.group == .system } }
    public var user: [SoundEntry] { entries.filter { $0.group == .user } }

    public func title(of choice: SoundChoice) -> String {
        entries.first { $0.choice == choice }?.title ?? SoundCatalog.title(file: choice.fileName ?? "")
    }

    static func title(file: String) -> String {
        (file as NSString).deletingPathExtension
    }

    private static func entries(
        _ files: [String], group: SoundGroup, make: (String) -> SoundChoice
    ) -> [SoundEntry] {
        let unique: Set<String> = Set(files.filter(isSoundFile))
        let kept: [String] = unique.sorted { (left: String, right: String) -> Bool in
            let a: String = title(file: left).lowercased(), b: String = title(file: right).lowercased()
            if a == b { return left < right }
            return a < b
        }
        return kept.map { (file: String) -> SoundEntry in
            SoundEntry(choice: make(file), group: group, title: title(file: file))
        }
    }

    private static func isSoundFile(_ name: String) -> Bool {
        guard !name.isEmpty, !name.hasPrefix("."), !name.contains("/") else { return false }
        return extensions.contains((name as NSString).pathExtension.lowercased())
    }
}

/// Where the preferences are kept; `UserDefaults` in the app, a dictionary in tests.
public protocol SoundStore: AnyObject {
    func data(forKey key: String) -> Data?
    func set(_ data: Data?, forKey key: String)
}

public final class UserDefaultsSoundStore: SoundStore, @unchecked Sendable {
    private let defaults: UserDefaults

    public init(_ defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func data(forKey key: String) -> Data? {
        defaults.data(forKey: key)
    }

    public func set(_ data: Data?, forKey key: String) {
        if let data { defaults.set(data, forKey: key) } else { defaults.removeObject(forKey: key) }
    }
}

/// The sound for notifications: one default and an optional choice per kind of waiting item.
public struct SoundPreferences: Codable, Equatable, Sendable {
    public static let key = "sounds.v1"
    /// The sound of last resort, when the default is a user sound that is gone.
    public static let builtIn: SoundChoice = .system("Glass.aiff")

    public var defaultSound: SoundChoice
    /// Keyed by the wire name of the kind; a kind without an entry uses the default.
    public var perKind: [String: SoundChoice]

    public init(defaultSound: SoundChoice = SoundPreferences.builtIn, perKind: [String: SoundChoice] = [:]) {
        self.defaultSound = defaultSound
        self.perKind = perKind
    }

    /// Reads the stored preferences; defaults when nothing is stored or it cannot be read.
    public static func load(from store: SoundStore) -> SoundPreferences {
        store.data(forKey: key).flatMap { try? JSONDecoder().decode(SoundPreferences.self, from: $0) }
            ?? SoundPreferences()
    }

    public func save(to store: SoundStore) {
        store.set(try? JSONEncoder().encode(self), forKey: SoundPreferences.key)
    }

    /// The choice the person made for the kind, before any file is checked.
    public func choice(for kind: WaitingKind) -> SoundChoice {
        perKind[kind.wireName] ?? defaultSound
    }

    /// The sound to play: nil for None. A user sound that `fileCheck` rejects (the file name is the argument) falls
    /// back to the default, and the default to the built-in sound.
    public func resolve(_ kind: WaitingKind, fileCheck: (String) -> Bool) -> SoundChoice? {
        var candidates = [choice(for: kind), defaultSound]
        candidates.append(SoundPreferences.builtIn)
        for candidate in candidates {
            switch candidate {
            case .silent: return nil
            case .system: return candidate
            case .user(let name): if fileCheck(name) { return candidate }
            }
        }
        return SoundPreferences.builtIn
    }
}

/// What the preview button can do for a choice.
public enum PreviewState: Equatable, Sendable {
    case playable
    /// None: there is nothing to play.
    case disabled
    /// The file is gone or cannot be decoded.
    case unavailable

    /// `fileCheck` answers whether the choice's file exists and decodes.
    public static func make(_ choice: SoundChoice, fileCheck: (SoundChoice) -> Bool) -> PreviewState {
        if choice == .silent { return .disabled }
        return fileCheck(choice) ? .playable : .unavailable
    }
}
