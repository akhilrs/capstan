import AudioToolbox
import CapstanAppCore
import CapstanKit
import Foundation

/// The phone's notification sounds: the system alert tones read at run time, and tones the person added, kept in the
/// app's Library/Sounds (the folder notifications read their sounds from).
@MainActor
enum PhoneSounds {
    static let systemDirectory = URL(fileURLWithPath: "/System/Library/Audio/UISounds", isDirectory: true)

    static var userDirectory: URL {
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return library.appendingPathComponent("Sounds", isDirectory: true)
    }

    static func catalog() -> SoundCatalog {
        SoundCatalog(systemFiles: files(in: systemDirectory), userFiles: files(in: userDirectory))
    }

    private static func files(in directory: URL) -> [String] {
        (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    }

    static func url(_ choice: SoundChoice) -> URL? {
        switch choice {
        case .silent: return nil
        case .system(let name): return systemDirectory.appendingPathComponent(name, isDirectory: false)
        case .user(let name): return userDirectory.appendingPathComponent(name, isDirectory: false)
        }
    }

    static func exists(_ choice: SoundChoice) -> Bool {
        guard let url = url(choice) else { return false }
        return FileManager.default.fileExists(atPath: url.path)
    }

    /// Plays a tone once at the alert volume; false when there is nothing to play.
    @discardableResult
    static func play(_ choice: SoundChoice) -> Bool {
        guard let url = url(choice), FileManager.default.fileExists(atPath: url.path) else { return false }
        var id: SystemSoundID = 0
        guard AudioServicesCreateSystemSoundID(url as CFURL, &id) == noErr else { return false }
        let sound = id
        AudioServicesPlayAlertSoundWithCompletion(sound) { AudioServicesDisposeSystemSoundID(sound) }
        return true
    }

    enum ImportError: Error {
        case notASound
        case unreadable
    }

    /// Copies a tone the person picked in Files into Library/Sounds; the file name stays, an older copy is replaced.
    static func importTone(from source: URL) throws -> String {
        let name = source.lastPathComponent
        guard SoundCatalog.extensions.contains(source.pathExtension.lowercased()), !name.hasPrefix(".") else {
            throw ImportError.notASound
        }
        let scoped = source.startAccessingSecurityScopedResource()
        defer { if scoped { source.stopAccessingSecurityScopedResource() } }
        let manager = FileManager.default
        try manager.createDirectory(at: userDirectory, withIntermediateDirectories: true)
        let target = userDirectory.appendingPathComponent(name, isDirectory: false)
        do {
            if manager.fileExists(atPath: target.path) { try manager.removeItem(at: target) }
            try manager.copyItem(at: source, to: target)
        } catch {
            throw ImportError.unreadable
        }
        return name
    }
}

/// The names of the kinds in Settings.
extension WaitingKind {
    var settingsName: String {
        switch self {
        case .op: return "Command proposals"
        case .prompt: return "Permission prompts"
        case .design: return "Design approvals"
        case .message: return "Messages from a PM"
        case .integration: return "Ready to merge"
        case .other(let name): return name
        }
    }

    /// The kinds Settings lists, in the order of the Waiting list.
    static var settingsKinds: [WaitingKind] { [.op, .prompt, .design, .message, .integration] }
}
