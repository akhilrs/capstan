import AppKit
import CapstanAppCore
import Foundation

/// The sound folders, read at run time, and the check that a sound is there and can be decoded.
enum SoundLibrary {
    static let systemDirectory = URL(fileURLWithPath: "/System/Library/Sounds", isDirectory: true)

    static var userDirectory: URL {
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return library.appendingPathComponent("Sounds", isDirectory: true)
    }

    static func catalog() -> SoundCatalog {
        SoundCatalog(systemFiles: listing(systemDirectory), userFiles: listing(userDirectory))
    }

    private static func listing(_ directory: URL) -> [String] {
        (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
    }

    static func url(_ choice: SoundChoice) -> URL? {
        switch choice {
        case .silent: return nil
        case .system(let name): return systemDirectory.appendingPathComponent(name, isDirectory: false)
        case .user(let name): return userDirectory.appendingPathComponent(name, isDirectory: false)
        }
    }

    /// True when the file is readable and the system can decode it.
    static func usable(_ choice: SoundChoice) -> Bool {
        guard let url = url(choice), FileManager.default.isReadableFile(atPath: url.path) else { return false }
        return NSSound(contentsOf: url, byReference: true) != nil
    }

    static func userFileUsable(_ name: String) -> Bool {
        usable(.user(name))
    }
}

/// Plays one sound at a time for the preview buttons.
@MainActor @Observable
final class SoundPlayer: NSObject, NSSoundDelegate {
    private(set) var playing: SoundChoice?
    @ObservationIgnored private var sound: NSSound?

    func toggle(_ choice: SoundChoice) {
        if playing == choice {
            stop()
        } else {
            play(choice)
        }
    }

    func play(_ choice: SoundChoice) {
        stop()
        guard let url = SoundLibrary.url(choice), let next = NSSound(contentsOf: url, byReference: true) else { return }
        next.delegate = self
        sound = next
        playing = choice
        if !next.play() { stop() }
    }

    func stop() {
        sound?.delegate = nil
        sound?.stop()
        sound = nil
        playing = nil
    }

    nonisolated func sound(_ sound: NSSound, didFinishPlaying flag: Bool) {
        Task { @MainActor in
            self.stop()
        }
    }
}
