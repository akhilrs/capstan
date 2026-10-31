import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

/// Every call the fakes saw, in order.
final class CallLog: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String] = []

    func add(_ item: String) { lock.withLock { items.append(item) } }
    var all: [String] { lock.withLock { items } }
}

final class FakeTransport: TransportControl, @unchecked Sendable {
    let events: AsyncStream<TransportEvent>
    private let sink: AsyncStream<TransportEvent>.Continuation
    private let log: CallLog
    private let lock = NSLock()
    private var latest: [UUID: TransportEvent] = [:]

    init(log: CallLog) {
        self.log = log
        let (stream, sink) = AsyncStream<TransportEvent>.makeStream(bufferingPolicy: .unbounded)
        events = stream
        self.sink = sink
    }

    func send(_ event: TransportEvent, hostId: UUID) {
        lock.withLock { latest[hostId] = event }
        sink.yield(event)
    }

    func latestEvent(hostId: UUID) -> TransportEvent? { lock.withLock { latest[hostId] } }
    func suspend() { log.add("suspend") }
}

final class FakeSession: HostSessionControl, @unchecked Sendable {
    private let name: String
    private let log: CallLog

    init(name: String, log: CallLog) {
        self.name = name
        self.log = log
    }

    func start() async { log.add("start \(name)") }
    func stop() async { log.add("stop \(name)") }
    func restart() async { log.add("restart \(name)") }
}

@MainActor
final class CoordinatorFixture {
    let directory: URL
    let pins: HostKeyStore
    let registry: PhoneHostRegistry
    let phoneStore: PhoneHostStore
    let hostsFile: HostsFile
    let log: CallLog
    let transport: FakeTransport
    let coordinator: HostsCoordinator
    /// What the next check reports.
    let steps: CheckSteps

    init(test: XCTestCase) throws {
        directory = try makeTempDirectory(test)
        pins = HostKeyStore(directory: directory)
        registry = PhoneHostRegistry(store: pins)
        phoneStore = PhoneHostStore(directory: directory)
        hostsFile = HostsFile(directory: directory)
        let log = CallLog()
        let steps = CheckSteps()
        self.log = log
        self.steps = steps
        transport = FakeTransport(log: log)
        coordinator = HostsCoordinator(
            transport: transport, registry: registry, pins: pins, phoneStore: phoneStore, hostsFile: hostsFile,
            makeSession: { config in FakeSession(name: config.displayName, log: log) },
            checker: { _ in
                let reported = steps.value
                return AsyncStream { continuation in
                    for step in reported { continuation.yield(step) }
                    continuation.finish()
                }
            })
    }

    final class CheckSteps: @unchecked Sendable {
        private let lock = NSLock()
        private var steps: [HostCheck] = []
        var value: [HostCheck] {
            get { lock.withLock { steps } }
            set { lock.withLock { steps = newValue } }
        }
    }

    /// Saves a host as if an earlier launch had added it.
    @discardableResult
    func saveHost(_ name: String, hostname: String? = nil, port: Int = 22) throws -> HostConfig {
        let id = UUID()
        let host = PhoneHost(id: id, hostname: hostname ?? name.lowercased(), port: port, user: "dev")
        let config = HostConfig(id: id, displayName: name, destination: host.alias)
        try phoneStore.save(phoneStore.load() + [host])
        try hostsFile.save(hostsFile.load() + [config])
        return config
    }

    func pinFor(_ id: UUID, _ byte: UInt8 = 1) -> HostKeyPin {
        HostKeyPin(algorithm: "ssh-ed25519", blob: Data(repeating: byte, count: 51))
    }

    /// Waits for the coordinator's event listener and its queued work.
    func settle(until condition: @MainActor () -> Bool = { true }) async {
        for _ in 0..<200 {
            await coordinator.waitForIdle()
            if condition() { return }
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
        await coordinator.waitForIdle()
    }
}

@MainActor
final class CoordinatorTests: XCTestCase {
    private let passed = [
        HostCheck(step: .connect, status: .passed, text: "Connected"),
        HostCheck(step: .capstan, status: .passed, text: "cstan found"),
    ]

    func testLaunchDropsHalfSavedHostsAndSavesTheCleanedLists() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let both = try fixture.saveHost("Both")
        let phoneOnly = PhoneHost(id: UUID(), hostname: "phone-only", user: "dev")
        try fixture.phoneStore.save(fixture.phoneStore.load() + [phoneOnly])
        try fixture.hostsFile.save(fixture.hostsFile.load() + [HostConfig(id: UUID(), displayName: "ConfigOnly", destination: "capstan-000000000001")])

        fixture.coordinator.start()
        await fixture.settle()

        XCTAssertEqual(fixture.coordinator.hosts.map(\.id), [both.id])
        XCTAssertEqual(fixture.hostsFile.load().map(\.id), [both.id])
        XCTAssertEqual(fixture.phoneStore.load().map(\.id), [both.id])
        XCTAssertEqual(fixture.log.all, ["start Both"])
    }

    func testAddHostCheckPassesThenCommit() async throws {
        let fixture = try CoordinatorFixture(test: self)
        fixture.coordinator.start()
        fixture.steps.value = passed
        let candidate = try fixture.coordinator.prepareCandidate(
            displayName: "Box", hostname: "box.example", port: 22, user: "dev", pollMs: 2000)
        XCTAssertEqual(fixture.registry.resolve(candidate.config.destination), candidate.host)

        var seen: [HostCheck] = []
        for await step in fixture.coordinator.check(candidate.config) { seen.append(step) }
        XCTAssertEqual(seen, passed)

        fixture.coordinator.commit(candidate)
        await fixture.settle()
        XCTAssertEqual(fixture.coordinator.hosts.map(\.id), [candidate.config.id])
        XCTAssertEqual(fixture.phoneStore.load(), [candidate.host])
        XCTAssertEqual(fixture.hostsFile.load().map(\.id), [candidate.config.id])
        XCTAssertEqual(fixture.log.all, ["start Box"])
    }

    func testAFailedCheckIsDiscardedAndItsPinsForgotten() async throws {
        let fixture = try CoordinatorFixture(test: self)
        fixture.coordinator.start()
        let candidate = try fixture.coordinator.prepareCandidate(
            displayName: "Box", hostname: "box.example", port: 22, user: "dev", pollMs: 2000)
        fixture.pins.trust(hostId: candidate.host.id, pin: fixture.pinFor(candidate.host.id))
        fixture.coordinator.discard(candidate)
        XCTAssertNil(fixture.pins.pin(for: candidate.host.id))
        XCTAssertNil(fixture.registry.resolve(candidate.config.destination))
        XCTAssertEqual(fixture.coordinator.hosts, [])
        XCTAssertEqual(fixture.phoneStore.load(), [])
    }

    func testADuplicateIsRefused() async throws {
        let fixture = try CoordinatorFixture(test: self)
        try fixture.saveHost("Box", hostname: "box.example")
        fixture.coordinator.start()
        XCTAssertThrowsError(try fixture.coordinator.prepareCandidate(
            displayName: "Again", hostname: "BOX.example.", port: 22, user: "dev", pollMs: 2000)) {
            XCTAssertEqual($0 as? PhoneHostError, .duplicate)
        }
        XCTAssertThrowsError(try fixture.coordinator.prepareCandidate(
            displayName: "Slow", hostname: "other", port: 22, user: "dev", pollMs: 999)) {
            XCTAssertEqual($0 as? HostConfigError, .pollMs)
        }
    }

    func testRemoveHostStopsItForgetsPinsAndSaves() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        fixture.pins.trust(hostId: box.id, pin: fixture.pinFor(box.id))
        fixture.coordinator.start()
        await fixture.settle()
        fixture.coordinator.removeHost(box.id)
        await fixture.settle()
        XCTAssertNil(fixture.pins.pin(for: box.id))
        XCTAssertEqual(fixture.coordinator.hosts, [])
        XCTAssertEqual(fixture.hostsFile.load(), [])
        XCTAssertEqual(fixture.phoneStore.load(), [])
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box"])
    }

    func testAnUnknownKeyBeforeLaunchGivesPendingTrustAndNoStart() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        let pin = fixture.pinFor(box.id)
        // Sent before the coordinator subscribed.
        fixture.transport.send(.hostKeyUnknown(hostId: box.id, pin: pin), hostId: box.id)
        fixture.coordinator.start()
        await fixture.settle()
        XCTAssertEqual(fixture.coordinator.pendingTrust[box.id], PendingTrust(hostId: box.id, pin: pin))
        XCTAssertEqual(fixture.coordinator.problems[box.id], .hostKeyUnknown(pin))
        XCTAssertEqual(fixture.log.all, [], "no session starts before the person trusts the key")
    }

    func testAnUnknownKeyWhileRunningStopsTheHostAndTrustingRestartsIt() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        fixture.coordinator.start()
        await fixture.settle()
        let pin = fixture.pinFor(box.id)
        fixture.transport.send(.hostKeyUnknown(hostId: box.id, pin: pin), hostId: box.id)
        await fixture.settle { fixture.coordinator.pendingTrust[box.id] != nil }
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box"])

        fixture.coordinator.trust(hostId: box.id)
        await fixture.settle()
        XCTAssertEqual(fixture.pins.pin(for: box.id), pin)
        XCTAssertNil(fixture.coordinator.pendingTrust[box.id])
        XCTAssertNil(fixture.coordinator.problems[box.id])
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box", "start Box"])
    }

    func testAChangedKeyBlocksRestart() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        fixture.coordinator.start()
        await fixture.settle()
        fixture.transport.send(.hostKeyChanged(hostId: box.id), hostId: box.id)
        await fixture.settle { fixture.coordinator.problems[box.id] == .hostKeyChanged }
        fixture.coordinator.restart(hostId: box.id)
        fixture.coordinator.startSessions()
        await fixture.settle()
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box"])
        XCTAssertEqual(fixture.coordinator.problems[box.id], .hostKeyChanged)
    }

    func testAuthRefusedStopsUntilTryAgain() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        fixture.coordinator.start()
        await fixture.settle()
        fixture.transport.send(.authRefused(hostId: box.id), hostId: box.id)
        await fixture.settle { fixture.coordinator.problems[box.id] == .authRefused }
        fixture.coordinator.restart(hostId: box.id)
        await fixture.settle()
        XCTAssertNil(fixture.coordinator.problems[box.id])
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box", "start Box"])
    }

    func testUnreadablePinsBlockEveryHostUntilReset() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let one = try fixture.saveHost("One")
        let two = try fixture.saveHost("Two")
        fixture.coordinator.start()
        await fixture.settle()
        fixture.transport.send(.pinsUnreadable, hostId: one.id)
        await fixture.settle { fixture.coordinator.pinsUnreadable }
        fixture.coordinator.restart(hostId: one.id)
        fixture.coordinator.startSessions()
        await fixture.settle()
        XCTAssertEqual(fixture.coordinator.runningHosts, [])
        XCTAssertEqual(fixture.coordinator.problems[two.id], .pinsUnreadable)

        fixture.coordinator.resetAllPins()
        await fixture.settle()
        XCTAssertFalse(fixture.coordinator.pinsUnreadable)
        XCTAssertEqual(fixture.coordinator.runningHosts, [one.id, two.id])
    }

    func testStopSessionsThenSuspendKeepsTheOrder() async throws {
        let fixture = try CoordinatorFixture(test: self)
        try fixture.saveHost("One")
        try fixture.saveHost("Two")
        fixture.coordinator.start()
        await fixture.settle()
        fixture.coordinator.stopSessionsThenSuspend()
        await fixture.settle()
        let tail = Array(fixture.log.all.dropFirst(2))
        XCTAssertEqual(Set(tail.prefix(2)), ["stop One", "stop Two"])
        XCTAssertEqual(tail.last, "suspend", "suspend comes after every stop")
        XCTAssertEqual(fixture.coordinator.runningHosts, [])

        fixture.coordinator.startSessions()
        await fixture.settle()
        XCTAssertEqual(fixture.coordinator.runningHosts.count, 2)
    }

    func testOnChangeFollowsTheCoordinatorsState() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box")
        var calls = 0
        fixture.coordinator.onChange = { calls += 1 }
        fixture.coordinator.start()
        XCTAssertGreaterThan(calls, 0, "the hosts were loaded")
        let afterStart = calls
        fixture.transport.send(.authRefused(hostId: box.id), hostId: box.id)
        await fixture.settle { fixture.coordinator.problems[box.id] == .authRefused }
        XCTAssertGreaterThan(calls, afterStart, "a problem was noted")
    }

    func testUpdateHostToAnotherPlaceForgetsItsPin() async throws {
        let fixture = try CoordinatorFixture(test: self)
        let box = try fixture.saveHost("Box", hostname: "box.example")
        fixture.pins.trust(hostId: box.id, pin: fixture.pinFor(box.id))
        fixture.coordinator.start()
        await fixture.settle()

        var renamed = box
        renamed.displayName = "Box"
        try fixture.coordinator.updateHost(renamed, host: PhoneHost(id: box.id, hostname: "box.example", user: "dev"))
        await fixture.settle()
        XCTAssertNotNil(fixture.pins.pin(for: box.id), "same place: the pin stays")

        try fixture.coordinator.updateHost(renamed, host: PhoneHost(id: box.id, hostname: "new.example", user: "dev"))
        await fixture.settle()
        XCTAssertNil(fixture.pins.pin(for: box.id), "another place: trusted again")
        XCTAssertEqual(fixture.phoneStore.load().first?.hostname, "new.example")
        XCTAssertEqual(fixture.log.all, ["start Box", "stop Box", "start Box", "stop Box", "start Box"])
    }
}
