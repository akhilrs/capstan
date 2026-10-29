import Foundation

enum RepoPaths {
    /// The repository root: this file is seven path components below it.
    static let repoRoot: URL = {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<7 { url.deleteLastPathComponent() }
        return url
    }()
}
