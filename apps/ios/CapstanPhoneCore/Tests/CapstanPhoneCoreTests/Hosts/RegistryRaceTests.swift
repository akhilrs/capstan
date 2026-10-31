import Foundation
import XCTest
@testable import CapstanPhoneCore

/// Resolves from 8 threads while commit and discard run. Run under `--sanitize=thread` too.
final class RegistryRaceTests: XCTestCase {
    private final class Failures: @unchecked Sendable {
        private let lock = NSLock()
        private var items: [String] = []

        func add(_ item: String) { lock.withLock { items.append(item) } }
        var all: [String] { lock.withLock { items } }
    }

    func testResolvesWhileCommitAndDiscardRun() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        let registry = PhoneHostRegistry(store: store)
        var hosts: [PhoneHost] = []
        for index in 0..<100 {
            let host = PhoneHost(id: registry.newHostId(), hostname: "box\(index)", user: "dev")
            try registry.registerCandidate(host)
            hosts.append(host)
        }
        let all = hosts
        let toCommit = Array(all[0..<50])
        let toDiscard = Array(all[50..<100])
        let committed = Set(toCommit.map(\.id))
        let failures = Failures()
        let resolvers = 8
        let resolvesPerThread = 10_000 / resolvers

        DispatchQueue.concurrentPerform(iterations: resolvers + 1) { worker in
            if worker == resolvers {
                for (commit, discard) in zip(toCommit, toDiscard) {
                    registry.commit(commit.id)
                    registry.discard(discard.id)
                }
                return
            }
            for step in 0..<resolvesPerThread {
                let host = all[(step * 7 + worker) % all.count]
                let resolved = registry.resolve(host.alias)
                // A committed host always resolves; a discarded one resolves to itself or to nothing.
                if resolved != nil && resolved != host { failures.add("\(host.alias) resolved to another host") }
                if committed.contains(host.id) && resolved == nil { failures.add("\(host.alias) vanished") }
            }
        }

        XCTAssertEqual(failures.all, [])
        for host in toCommit { XCTAssertEqual(registry.resolve(host.alias), host) }
        for host in toDiscard { XCTAssertNil(registry.resolve(host.alias)) }
        XCTAssertEqual(Set(registry.savedHosts.map(\.id)), Set(toCommit.map(\.id)))
    }
}
