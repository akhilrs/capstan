import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class HostsTests: XCTestCase {
    private func makeRegistry() throws -> (PhoneHostRegistry, HostKeyStore) {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        return (PhoneHostRegistry(store: store), store)
    }

    // MARK: Validation

    func testValidation() {
        let id = UUID()
        XCTAssertTrue(PhoneHost(id: id, hostname: "build-box.example", user: "dev").validate().isEmpty)
        XCTAssertTrue(PhoneHost(id: id, hostname: "192.168.1.20", port: 2222, user: "dev").validate().isEmpty)
        XCTAssertTrue(PhoneHost(id: id, hostname: "fe80::1%en0", user: "dev_1.x").validate().isEmpty)

        XCTAssertEqual(PhoneHost(id: id, hostname: "", user: "dev").validate(), [.hostname])
        XCTAssertEqual(PhoneHost(id: id, hostname: "-oProxyCommand=x", user: "dev").validate(), [.hostname])
        XCTAssertEqual(PhoneHost(id: id, hostname: "a b", user: "dev").validate(), [.hostname])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box;rm", user: "dev").validate(), [.hostname])
        XCTAssertEqual(PhoneHost(id: id, hostname: String(repeating: "a", count: 254), user: "dev").validate(), [.hostname])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", port: 0, user: "dev").validate(), [.port])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", port: 65536, user: "dev").validate(), [.port])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", user: "").validate(), [.user])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", user: "-x").validate(), [.user])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", user: "a b").validate(), [.user])
        XCTAssertEqual(PhoneHost(id: id, hostname: "box", user: "dev").port, 22)
    }

    func testAliasIsValidForHostConfigAndShowsNothingOfTheHost() throws {
        let id = try XCTUnwrap(UUID(uuidString: "E621E1F8-C36C-495A-93FC-0C247A3E6E5F"))
        XCTAssertEqual(PhoneHost.alias(for: id), "capstan-e621e1f8c36c")
        let config = HostConfig(id: id, displayName: "Box", destination: PhoneHost.alias(for: id))
        XCTAssertTrue(config.validate().isEmpty)
    }

    func testDisplay() {
        XCTAssertEqual(PhoneHost(id: UUID(), hostname: "box", port: 2222, user: "dev").display, "box:2222")
        XCTAssertEqual(PhoneHost(id: UUID(), hostname: "fe80::1", user: "dev").display, "[fe80::1]:22")
    }

    // MARK: Registry

    func testAliasLookup() throws {
        let (registry, _) = try makeRegistry()
        let host = PhoneHost(id: UUID(), hostname: "box", user: "dev")
        try registry.register(host)
        XCTAssertEqual(registry.resolve(host.alias), host)
        XCTAssertNil(registry.resolve("capstan-000000000000"))
        XCTAssertNil(registry.resolve("box"), "only aliases resolve")
    }

    func testDuplicateIsRefusedIgnoringCaseAndTrailingDot() throws {
        let (registry, _) = try makeRegistry()
        try registry.register(PhoneHost(id: UUID(), hostname: "Box.Example.", user: "dev"))
        XCTAssertThrowsError(try registry.registerCandidate(PhoneHost(id: UUID(), hostname: "box.example", user: "dev"))) {
            XCTAssertEqual($0 as? PhoneHostError, .duplicate)
        }
        XCTAssertThrowsError(try registry.register(PhoneHost(id: UUID(), hostname: "BOX.EXAMPLE", user: "dev"))) {
            XCTAssertEqual($0 as? PhoneHostError, .duplicate)
        }
        XCTAssertNoThrow(try registry.registerCandidate(PhoneHost(id: UUID(), hostname: "box.example", user: "ops")))
    }

    func testTwoPortsOnOneHost() throws {
        let (registry, _) = try makeRegistry()
        let first = PhoneHost(id: UUID(), hostname: "box", port: 22, user: "dev")
        let second = PhoneHost(id: UUID(), hostname: "box", port: 2222, user: "dev")
        try registry.register(first)
        try registry.register(second)
        XCTAssertEqual(registry.resolve(first.alias)?.port, 22)
        XCTAssertEqual(registry.resolve(second.alias)?.port, 2222)
    }

    func testInvalidHostIsRefused() throws {
        let (registry, _) = try makeRegistry()
        XCTAssertThrowsError(try registry.registerCandidate(PhoneHost(id: UUID(), hostname: "box", port: 0, user: "dev"))) {
            XCTAssertEqual($0 as? PhoneHostError, .port)
        }
    }

    func testAliasInUseAsksForANewId() throws {
        let (registry, _) = try makeRegistry()
        let first = try XCTUnwrap(UUID(uuidString: "12345678-9ABC-4000-8000-000000000001"))
        let second = try XCTUnwrap(UUID(uuidString: "12345678-9ABC-4000-8000-000000000002"))
        XCTAssertEqual(PhoneHost.alias(for: first), PhoneHost.alias(for: second))
        try registry.register(PhoneHost(id: first, hostname: "one", user: "dev"))
        XCTAssertThrowsError(try registry.registerCandidate(PhoneHost(id: second, hostname: "two", user: "dev"))) {
            XCTAssertEqual($0 as? PhoneHostError, .aliasInUse)
        }
        let fresh = registry.newHostId()
        XCTAssertNotEqual(PhoneHost.alias(for: fresh), PhoneHost.alias(for: first))
        XCTAssertNoThrow(try registry.registerCandidate(PhoneHost(id: fresh, hostname: "two", user: "dev")))
    }

    func testCandidateResolveCommitDiscard() throws {
        let (registry, store) = try makeRegistry()
        let kept = PhoneHost(id: registry.newHostId(), hostname: "kept", user: "dev")
        let dropped = PhoneHost(id: registry.newHostId(), hostname: "dropped", user: "dev")
        try registry.registerCandidate(kept)
        try registry.registerCandidate(dropped)
        store.trust(hostId: kept.id, pin: makePin())
        store.trust(hostId: dropped.id, pin: makePin())
        XCTAssertEqual(registry.resolve(kept.alias), kept)
        XCTAssertEqual(registry.resolve(dropped.alias), dropped)
        XCTAssertTrue(registry.savedHosts.isEmpty)

        registry.commit(kept.id)
        registry.discard(dropped.id)
        XCTAssertEqual(registry.resolve(kept.alias), kept)
        XCTAssertEqual(registry.savedHosts, [kept])
        XCTAssertNil(registry.resolve(dropped.alias))
        XCTAssertNil(store.pin(for: dropped.id), "a discarded candidate's pins are forgotten")
        XCTAssertEqual(store.pin(for: kept.id), makePin())

        registry.discard(kept.id)
        XCTAssertEqual(registry.resolve(kept.alias), kept, "discard does not touch a committed host")
    }

    func testResolvedCopyOutlivesDiscard() throws {
        let (registry, _) = try makeRegistry()
        let host = PhoneHost(id: registry.newHostId(), hostname: "box", user: "dev")
        try registry.registerCandidate(host)
        let resolved = registry.resolve(host.alias)
        registry.discard(host.id)
        XCTAssertEqual(resolved, host)
        XCTAssertNil(registry.resolve(host.alias))
    }

    func testRemoveForgetsPins() throws {
        let (registry, store) = try makeRegistry()
        let host = PhoneHost(id: UUID(), hostname: "box", user: "dev")
        try registry.register(host)
        store.trust(hostId: host.id, pin: makePin())
        registry.remove(host.id)
        XCTAssertNil(registry.resolve(host.alias))
        XCTAssertNil(store.pin(for: host.id))
    }
}
