import Foundation
@testable import CapstanKit

struct KernelFixture {
    let name: String
    let data: Data
}

enum KernelFixtures {
    static let directory = RepoPaths.repoRoot
        .appendingPathComponent("rust/crates/kernel/tests/rust_only/fixtures/event_feed")

    /// Every event fixture, by file name without extension, with the placeholders filled in:
    /// `"<id:N>"` becomes a fixed identifier and `"<ts>"` a fixed time.
    static func events() throws -> [KernelFixture] {
        let names = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            .filter { $0.hasSuffix(".json") }
            .sorted()
        return try names.map { file in
            var text = try String(contentsOf: directory.appendingPathComponent(file), encoding: .utf8)
            for number in 1...9 {
                text = text.replacingOccurrences(
                    of: "\"<id:\(number)>\"", with: "\"a1b2c3d4-0000-4000-8000-00000000000\(number)\"")
            }
            text = text.replacingOccurrences(of: "\"<ts>\"", with: "\"2026-10-01T09:30:00.000Z\"")
            return KernelFixture(name: String(file.dropLast(".json".count)), data: Data(text.utf8))
        }
    }

    static func event(_ name: String) throws -> FeedEvent {
        guard let fixture = try events().first(where: { $0.name == name }) else {
            throw ShParseError(message: "no fixture \(name)")
        }
        return try JSONDecoder().decode(FeedEvent.self, from: fixture.data)
    }
}
