import Foundation
import XCTest
@testable import CapstanKit

final class PersistenceTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("store-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func completeProject(_ id: String = "proj-a", keys: [String] = ["op:7"]) throws -> [StreamLine] {
        var lines = [try StoreWire.project(id)]
        for key in keys {
            lines.append(try StoreWire.snapshot(id, "waiting", key: key, item: StoreWire.waitingItem(key)))
        }
        lines.append(try StoreWire.snapshot(id, "agents", key: "dev-1", item: StoreWire.agentItem("dev-1")))
        lines.append(try StoreWire.end(id))
        return lines
    }

    private func sampleModel() throws -> PersistedHost {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello()] + completeProject())
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("prompt.waiting")))
        let cursor = makeCursor(["proj-a"])
        harness.apply(try StoreWire.cursor(cursor))
        harness.host.forwards["proj-a"] = [ForwardRecord(requestId: "r", contentHash: "h", messageId: "m", at: Date(timeIntervalSince1970: 5))]
        return PersistedHost(host: harness.host, staging: harness.staging, savedAt: Date(timeIntervalSince1970: 1_234.5))
    }

    func testForwardsOfAProjectStillInStagingAreSavedWithinTheCap() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello()] + completeProject())
        harness.apply(
            try StoreWire.project("proj-b"),
            try StoreWire.snapshot("proj-b", "waiting", key: "op:1", item: StoreWire.waitingItem("op:1")))
        let records = (0..<(HostStoreEngine.maxForwardsPerProject + 3)).map {
            ForwardRecord(requestId: "r\($0)", contentHash: "h", at: Date(timeIntervalSince1970: Double($0)))
        }
        harness.host.forwards["proj-b"] = records
        let saved = PersistedHost(host: harness.host, staging: harness.staging, savedAt: Date())
        XCTAssertNil(saved.projects["proj-b"])
        XCTAssertEqual(saved.forwards["proj-b"], Array(records.suffix(HostStoreEngine.maxForwardsPerProject)))
    }

    func testSaveAndLoadGiveAnEqualModelAndCursor() throws {
        let store = FilePersistence(directory: directory)
        let saved = try sampleModel()
        try store.save(saved)
        let loaded = try XCTUnwrap(store.load(hostId: saved.hostId))
        XCTAssertEqual(loaded, saved)
        XCTAssertEqual(loaded.cursor, makeCursor(["proj-a"]))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("hosts/\(saved.hostId.uuidString).json").path))
    }

    func testACorruptOrOtherVersionFileGivesNil() throws {
        let store = FilePersistence(directory: directory)
        let saved = try sampleModel()
        let file = directory.appendingPathComponent("hosts/\(saved.hostId.uuidString).json")
        try store.save(saved)
        let text = try String(contentsOf: file, encoding: .utf8)
        XCTAssertTrue(text.contains("\"version\":2"))
        try text.replacingOccurrences(of: "\"version\":2", with: "\"version\":3").write(to: file, atomically: true, encoding: .utf8)
        XCTAssertNil(store.load(hostId: saved.hostId))
        try Data("{not json".utf8).write(to: file)
        XCTAssertNil(store.load(hostId: saved.hostId))
        XCTAssertNil(store.load(hostId: UUID()))
    }

    func testDeleteRemovesTheFile() throws {
        let store = FilePersistence(directory: directory)
        let saved = try sampleModel()
        try store.save(saved)
        try store.delete(hostId: saved.hostId)
        XCTAssertNil(store.load(hostId: saved.hostId))
        XCTAssertNoThrow(try store.delete(hostId: saved.hostId))
    }

    func testHostsFileKeepsTheConfigs() throws {
        let file = HostsFile(directory: directory)
        XCTAssertEqual(file.load(), [])
        let hosts = [makeConfig(), makeConfig(destination: "other@box")]
        try file.save(hosts)
        XCTAssertEqual(file.load(), hosts)
        try Data("nope".utf8).write(to: directory.appendingPathComponent("hosts.json"))
        XCTAssertEqual(file.load(), [])
    }

    func testANewEngineKeepsTheBaselineSoOnlyANewKeyNotifies() async throws {
        let hostId = UUID()
        let store = FilePersistence(directory: directory)
        let first = makeEngine(hostId: hostId, persistence: store)
        await first.apply(
            host: hostId, lines: try [StoreWire.hello()] + completeProject() + [StoreWire.cursor(makeCursor(["proj-a"]))])

        let recorder = EngineRecorder()
        let second = makeEngine(hostId: hostId, persistence: store, recorder: recorder)
        let loaded = await second.current()
        XCTAssertEqual(loaded.projects["proj-a"]?.waiting.count, 1)
        let resume = await second.resumeCursor(host: hostId)
        XCTAssertEqual(resume, makeCursor(["proj-a"]))
        await second.apply(
            host: hostId, lines: try [StoreWire.hello()] + completeProject(keys: ["op:7", "op:8"]))
        XCTAssertEqual(recorder.notifications.map(\.key), ["op:8"])
    }

    func testAProjectStillInStagingHasNoCursorEntryInTheSavedOrTheResumeCursor() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        let staged = try StoreWire.snapshot("proj-b", "waiting", key: "op:1", item: StoreWire.waitingItem("op:1"))
        await engine.apply(
            host: hostId,
            lines: try [StoreWire.hello()] + completeProject() + [StoreWire.project("proj-b"), staged]
                + [StoreWire.cursor(makeCursor(["proj-a", "proj-b"]))])
        let saved = try XCTUnwrap(store.stored)
        XCTAssertEqual(Set(try XCTUnwrap(saved.cursor).decoded().p.keys), ["proj-a"])
        XCTAssertEqual(Set(saved.projects.keys), ["proj-a"])
        let resume = await engine.resumeCursor(host: hostId)
        XCTAssertEqual(Set(try XCTUnwrap(resume).decoded().p.keys), ["proj-a"])
        let kept = await engine.current().cursor
        XCTAssertEqual(Set(try XCTUnwrap(kept).decoded().p.keys), ["proj-a", "proj-b"])
    }

    func testCursorLinesWithinTwoSecondsAreCoalescedIntoOneSaveWithTheNewestPair() async throws {
        let hostId = UUID()
        let clock = TestClock()
        let start = clock.now()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store, clock: clock)
        let cursors = (0..<4).map { makeCursor(["proj-a"], sequence: Int64(10 + $0)) }

        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject() + [StoreWire.cursor(cursors[0])])
        XCTAssertEqual(store.saves.count, 1, "the first cursor line saves at once")
        XCTAssertEqual(store.saves.first?.savedAt, start)

        for step in 1...3 {
            clock.advance(by: 0.3)
            await engine.apply(host: hostId, lines: [try StoreWire.cursor(cursors[step])])
            // The deferred save must be asleep before time moves on, or its wake time would drift.
            await eventually("the deferred save to sleep") { clock.sleeperCount == 1 }
        }
        await settle()
        XCTAssertEqual(store.saves.count, 1)

        clock.advance(by: 1.0)
        await settle()
        XCTAssertEqual(store.saves.count, 1, "1.9 s: not yet")

        clock.advance(by: 0.2)
        await eventually("the save at the 2 s mark") { store.saves.count == 2 }
        XCTAssertEqual(store.saves.last?.cursor, cursors[3])
        let savedAt = try XCTUnwrap(store.saves.last?.savedAt).timeIntervalSince(start)
        XCTAssertTrue(savedAt >= 2 && savedAt < 2.5, "saved at \(savedAt)")
    }

    func testADeferredSaveBuildsTheHostOnlyWhenItWrites() async throws {
        let hostId = UUID()
        let clock = TestClock()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store, clock: clock)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject() + [StoreWire.cursor(makeCursor(["proj-a"], sequence: 1))])
        let afterFirst = await engine.persistedBuildCount
        XCTAssertEqual(afterFirst, 1, "the first save builds and writes at once")

        for step in 2...4 {
            clock.advance(by: 0.3)
            await engine.apply(host: hostId, lines: [try StoreWire.cursor(makeCursor(["proj-a"], sequence: Int64(step)))])
            await eventually("the deferred save to sleep") { clock.sleeperCount == 1 }
        }
        let whileDeferred = await engine.persistedBuildCount
        XCTAssertEqual(whileDeferred, 1, "a deferred save builds nothing")
        XCTAssertEqual(store.saves.count, 1)

        clock.advance(by: 1.2)
        await eventually("the save at the 2 s mark") { store.saves.count == 2 }
        let afterWrite = await engine.persistedBuildCount
        XCTAssertEqual(afterWrite, 2, "the write builds it once")
        XCTAssertEqual(store.saves.last?.cursor, makeCursor(["proj-a"], sequence: 4))
    }

    func testFlushSavesAtOnce() async throws {
        let hostId = UUID()
        let clock = TestClock()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store, clock: clock)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject() + [StoreWire.cursor(makeCursor(["proj-a"], sequence: 1))])
        clock.advance(by: 0.5)
        await engine.apply(host: hostId, lines: [try StoreWire.cursor(makeCursor(["proj-a"], sequence: 2))])
        XCTAssertEqual(store.saves.count, 1)
        await engine.flush()
        XCTAssertEqual(store.saves.count, 2)
        XCTAssertEqual(store.saves.last?.cursor, makeCursor(["proj-a"], sequence: 2))
        clock.advance(by: 3)
        await settle()
        XCTAssertEqual(store.saves.count, 2, "the deferred save was cancelled")
    }

    func testDropCursorClearsTheCursorAndDeletesTheFile() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject() + [StoreWire.cursor(makeCursor(["proj-a"]))])
        XCTAssertNotNil(store.stored)
        await engine.dropCursor(host: hostId)
        let after = await engine.current()
        XCTAssertNil(after.cursor)
        XCTAssertNil(store.stored)
        XCTAssertEqual(store.deletes, 1)
        let resume = await engine.resumeCursor(host: hostId)
        XCTAssertNil(resume)
    }

    func testRecordForwardIsReadableAndSaved() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject())
        let record = ForwardRecord(requestId: "req-op:7", contentHash: "hash-1", messageId: "m1", at: Date(timeIntervalSince1970: 9))
        await engine.recordForward(projectId: "proj-a", record)
        let forwards = await engine.forwards(projectId: "proj-a")
        XCTAssertEqual(forwards, [record])
        let all = await engine.forwards()
        XCTAssertEqual(all, [record])
        let model = await engine.current()
        XCTAssertEqual(inbox([model]).first?.rows.first?.forwarded, true)
        XCTAssertEqual(store.stored?.forwards["proj-a"], [record])
    }

    func testLinesOfAnotherHostAreIgnored() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        await engine.apply(host: UUID(), lines: [try StoreWire.hello()])
        let model = await engine.current()
        XCTAssertNil(model.hello)
    }
}
