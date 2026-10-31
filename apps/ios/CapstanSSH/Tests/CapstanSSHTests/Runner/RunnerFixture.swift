import CapstanKit
import CapstanPhoneCore
import CryptoKit
import Foundation
import NIOSSH
import XCTest
@testable import CapstanSSH

/// Keys in memory, for a vault without the Keychain.
final class MemoryKeyStorage: KeyStorage, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [StoredKey] = []

    func all() throws -> [StoredKey] { lock.withLock { keys } }
    func add(_ key: StoredKey) throws { lock.withLock { keys.append(key) } }
    func remove(id: String) throws { lock.withLock { keys.removeAll { $0.id == id } } }
    func removeAll() throws { lock.withLock { keys = [] } }
}

/// A runner, its stores, a test server and one registered host pointing at it.
final class RunnerFixture {
    let directory: URL
    let pins: HostKeyStore
    let registry: PhoneHostRegistry
    let vault: KeychainKeyVault
    let clock = TestClock()
    let phoneKey: PhoneKey
    let server: TestSSHServer
    let host: PhoneHost
    let config: HostConfig
    let runner: SSHRunner

    /// `serverAccepts: false` makes the server refuse the phone's key. `pinsFile` is written before the pin store loads.
    init(
        trusted: Bool = true, serverAccepts: Bool = true, pinsFile: Data? = nil, hook: any TransportHook = DefaultHook(),
        port: Int? = nil
    ) throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("capstan-runner-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        if let pinsFile { try pinsFile.write(to: directory.appendingPathComponent("host-keys.json")) }
        pins = HostKeyStore(directory: directory)
        registry = PhoneHostRegistry(store: pins)
        vault = KeychainKeyVault(storage: MemoryKeyStorage())
        phoneKey = try vault.create(.keychainEd25519)
        let accepted = serverAccepts
            ? SSHRunner.nioKey(phoneKey).publicKey
            : NIOSSHPrivateKey(ed25519Key: Curve25519.Signing.PrivateKey()).publicKey
        server = try TestSSHServer(acceptedKey: accepted)
        host = PhoneHost(id: registry.newHostId(), hostname: "127.0.0.1", port: port ?? server.port, user: "tester")
        try registry.register(host)
        config = HostConfig(id: host.id, displayName: "Test", destination: host.alias)
        if trusted && pinsFile == nil { pins.trust(hostId: host.id, pin: server.hostPin) }
        runner = SSHRunner(
            registry: registry, vault: vault, trust: pins, tuning: TransportTuning(), clock: clock, hook: hook)
    }

    func start(_ command: String) throws -> any RunningProcess {
        try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), RemoteCommand.sshArguments(config, remote: command))
    }

    func stop() {
        runner.shutdown()
        server.stop()
        try? FileManager.default.removeItem(at: directory)
    }
}

/// Every line, and the error that ended the lines if any.
func collect(_ process: any RunningProcess) async -> (lines: [String], error: Error?) {
    var lines: [String] = []
    do {
        for try await line in process.lines { lines.append(String(decoding: line, as: UTF8.self)) }
        return (lines, nil)
    } catch {
        return (lines, error)
    }
}

/// The exit code, failing the test after `seconds` instead of hanging the run.
func exitCode(
    _ process: any RunningProcess, within seconds: TimeInterval = 10, file: StaticString = #filePath, line: UInt = #line
) async -> Int32? {
    await withTaskGroup(of: Int32?.self) { group in
        group.addTask { await process.exitCode() }
        group.addTask {
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            return nil
        }
        let first = await group.next() ?? nil
        group.cancelAll()
        if first == nil { XCTFail("no exit code within \(seconds) s", file: file, line: line) }
        return first
    }
}

/// Reads the runner's events as the app's coordinator does: one consumer, subscribed once.
final class EventRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [TransportEvent] = []
    private var task: Task<Void, Never>?

    init(_ events: AsyncStream<TransportEvent>) {
        task = Task { [weak self] in
            for await event in events { self?.add(event) }
        }
    }

    private func add(_ event: TransportEvent) { lock.withLock { items.append(event) } }

    var all: [TransportEvent] { lock.withLock { items } }

    /// Waits until `count` events arrived.
    func wait(for count: Int, timeout: TimeInterval = 5) async -> [TransportEvent] {
        await eventually(timeout: timeout, "\(count) transport events") { self.all.count >= count }
        return all
    }
}
