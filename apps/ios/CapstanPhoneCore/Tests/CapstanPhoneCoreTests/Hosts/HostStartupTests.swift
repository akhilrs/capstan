import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class HostStartupTests: XCTestCase {
    private func config(_ id: UUID) -> HostConfig {
        HostConfig(id: id, displayName: "Box", destination: PhoneHost.alias(for: id))
    }

    func testHalfSavedHostsAreDroppedAndTheCleanedListsSaved() throws {
        let directory = try makeTempDirectory(self)
        let hostsFile = HostsFile(directory: directory)
        let phoneStore = PhoneHostStore(directory: directory)
        let pins = HostKeyStore(directory: directory)
        let registry = PhoneHostRegistry(store: pins)

        let both = PhoneHost(id: UUID(), hostname: "both", user: "dev")
        let phoneOnly = PhoneHost(id: UUID(), hostname: "phone-only", user: "dev")
        let configOnly = UUID()
        try phoneStore.save([both, phoneOnly])
        try hostsFile.save([config(both.id), config(configOnly)])
        pins.trust(hostId: both.id, pin: makePin())
        pins.trust(hostId: phoneOnly.id, pin: makePin())

        let started = HostStartup.run(hostsFile: hostsFile, phoneStore: phoneStore, registry: registry, pins: pins)
        pins.waitForWrites()

        XCTAssertEqual(started.map(\.id), [both.id])
        XCTAssertEqual(hostsFile.load().map(\.id), [both.id], "the cleaned configs are saved")
        XCTAssertEqual(phoneStore.load(), [both], "the cleaned phone hosts are saved")
        XCTAssertEqual(registry.resolve(both.alias), both)
        XCTAssertNil(registry.resolve(phoneOnly.alias))
        XCTAssertEqual(pins.pin(for: both.id), makePin())
        XCTAssertNil(pins.pin(for: phoneOnly.id), "pins of hosts that are gone are swept")
    }

    func testMatchingListsAreKept() throws {
        let directory = try makeTempDirectory(self)
        let hostsFile = HostsFile(directory: directory)
        let phoneStore = PhoneHostStore(directory: directory)
        let pins = HostKeyStore(directory: directory)
        let registry = PhoneHostRegistry(store: pins)
        let first = PhoneHost(id: UUID(), hostname: "one", user: "dev")
        let second = PhoneHost(id: UUID(), hostname: "two", port: 2222, user: "dev")
        try phoneStore.save([first, second])
        try hostsFile.save([config(first.id), config(second.id)])

        let started = HostStartup.run(hostsFile: hostsFile, phoneStore: phoneStore, registry: registry, pins: pins)
        XCTAssertEqual(Set(started.map(\.id)), [first.id, second.id])
        XCTAssertEqual(Set(registry.savedHosts.map(\.id)), [first.id, second.id])
    }

    func testReconcileKeepsOnlyPairs() {
        let a = PhoneHost(id: UUID(), hostname: "a", user: "dev")
        let b = PhoneHost(id: UUID(), hostname: "b", user: "dev")
        let c = UUID()
        let result = HostStartup.reconcile(configs: [config(a.id), config(c)], phoneHosts: [a, b])
        XCTAssertEqual(result.configs.map(\.id), [a.id])
        XCTAssertEqual(result.phoneHosts, [a])
    }
}
