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

    func testTheActivityFieldsRoundTripAndAnOlderFileLoadsWithNil() throws {
        var harness = ReducerHarness()
        harness.apply(try [StoreWire.hello()] + completeProject())
        harness.apply(.event(projectId: "proj-a", try KernelFixtures.event("agent.activity")))
        let saved = PersistedHost(host: harness.host, staging: harness.staging, savedAt: Date(timeIntervalSince1970: 1))
        let data = try JSONEncoder().encode(saved)
        let loaded = try JSONDecoder().decode(PersistedHost.self, from: data)
        let agent = try XCTUnwrap(loaded.projects["proj-a"]?.agents["dev-1"])
        XCTAssertEqual(agent.activity, "working")
        XCTAssertEqual(agent.activitySince, "2026-01-01T00:00:00.033Z")
        XCTAssertEqual(agent.activitySeq, 1)

        var tree = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        var projects = try XCTUnwrap(tree["projects"] as? [String: Any])
        var project = try XCTUnwrap(projects["proj-a"] as? [String: Any])
        var agents = try XCTUnwrap(project["agents"] as? [String: Any])
        var old = try XCTUnwrap(agents["dev-1"] as? [String: Any])
        for name in ["activity", "activitySince", "activitySeq"] { old[name] = nil }
        agents["dev-1"] = old
        project["agents"] = agents
        projects["proj-a"] = project
        tree["projects"] = projects
        let older = try JSONDecoder().decode(
            PersistedHost.self, from: try JSONSerialization.data(withJSONObject: tree))
        let kept = try XCTUnwrap(older.projects["proj-a"]?.agents["dev-1"])
        XCTAssertNil(kept.activity)
        XCTAssertNil(kept.activitySince)
        XCTAssertNil(kept.activitySeq)
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

    // MARK: A run change not yet read

    private func runProject(_ id: String, hash: String = "run-hash-1", state: String = "active", paused: Bool = false)
        throws -> [StreamLine]
    {
        [
            try StoreWire.project(id),
            try StoreWire.snapshot(id, "run", key: "run", item: StoreWire.runItem(state: state, paused: paused, hash: hash)),
            try StoreWire.end(id),
        ]
    }

    private func runChanged(_ id: String) throws -> StreamLine {
        guard case .event(_, let event) = try StoreWire.runChanged(state: "paused", paused: true) else {
            throw XCTSkip("not an event")
        }
        return .event(projectId: id, event)
    }

    private func agentInvalidate(_ id: String) throws -> StreamLine {
        .event(projectId: id, try StoreWire.event(
            "snapshot.invalidated", seq: 40, effects: [("agents", "invalidate", "dev-1", "{}")]))
    }

    func testARunChangeNotYetReadIsNotSavedAndNotResumed() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(
            host: hostId,
            lines: try [StoreWire.hello()] + runProject("proj-a") + [StoreWire.cursor(makeCursor(["proj-a"]))])
        await engine.apply(host: hostId, lines: [try runChanged("proj-a"), try StoreWire.cursor(makeCursor(["proj-a"]))])
        await engine.flush()
        let saved = try XCTUnwrap(store.stored)
        XCTAssertNil(saved.projects["proj-a"], "a project with an unknown run hash is not saved")
        XCTAssertNil(saved.cursor, "and has no entry in the saved cursor")
        let resume = await engine.resumeCursor(host: hostId)
        XCTAssertNil(resume, "the reconnect of a live engine reads the project in full")

        let relaunched = makeEngine(hostId: hostId, persistence: store)
        let loaded = await relaunched.current()
        XCTAssertNil(loaded.projects["proj-a"], "a relaunch holds no run without a hash")
        let cursorAfterRelaunch = await relaunched.resumeCursor(host: hostId)
        XCTAssertNil(cursorAfterRelaunch)
    }

    func testTheFullSnapshotAfterARunChangeBringsTheHash() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + runProject("proj-a"))
        await engine.apply(host: hostId, lines: [try runChanged("proj-a")])
        let blank = await engine.current().projects["proj-a"]?.run
        XCTAssertEqual(blank?.hashKnown, false)
        await engine.apply(
            host: hostId, lines: try runProject("proj-a", hash: "run-hash-2", state: "paused", paused: true))
        let read = await engine.current().projects["proj-a"]?.run
        XCTAssertEqual(read?.contentHash, "run-hash-2")
        XCTAssertEqual(read?.paused, true)
    }

    func testARunChangeDuringAnArrivingSnapshotIsReplacedByTheSnapshotRun() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder, urgentCallback: true)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + runProject("proj-a"))
        let restart = try runProject("proj-a", hash: "run-hash-2", state: "paused", paused: true)
        await engine.apply(host: hostId, lines: [restart[0], restart[1], try runChanged("proj-a")])
        XCTAssertTrue(recorder.urgentRefreshes.isEmpty, "a snapshot is arriving: nothing to restart for")
        XCTAssertTrue(recorder.refreshes.isEmpty)
        await engine.apply(host: hostId, lines: [restart[2]])
        let run = await engine.current().projects["proj-a"]?.run
        XCTAssertEqual(run?.contentHash, "run-hash-2", "the snapshot that was arriving always re-sends the run")
    }

    // MARK: Engine refresh paths

    func testARunChangeGoesToTheUrgentCallbackAndOtherInvalidatesToTheRefreshCallback() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder, urgentCallback: true)
        let agent = try StoreWire.snapshot("proj-b", "agents", key: "dev-1", item: StoreWire.agentItem("dev-1"))
        await engine.apply(
            host: hostId,
            lines: try [StoreWire.hello()] + runProject("proj-a") + [StoreWire.project("proj-b"), agent, StoreWire.end("proj-b")])
        await engine.apply(host: hostId, lines: [try runChanged("proj-a"), try agentInvalidate("proj-b")])
        XCTAssertEqual(recorder.urgentRefreshes, [["proj-a"]])
        XCTAssertEqual(recorder.refreshes, [["proj-b"]])
    }

    func testAProjectWithBothKindsOfInvalidateIsOnlyUrgent() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder, urgentCallback: true)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + runProject("proj-a"))
        await engine.apply(host: hostId, lines: [try runChanged("proj-a"), try agentInvalidate("proj-a")])
        XCTAssertEqual(recorder.urgentRefreshes, [["proj-a"]])
        XCTAssertTrue(recorder.refreshes.isEmpty, "the urgent read covers the project")
    }

    func testWithoutAnUrgentCallbackARunChangeUsesTheRefreshCallback() async throws {
        let hostId = UUID()
        let recorder = EngineRecorder()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence(), recorder: recorder)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + runProject("proj-a"))
        await engine.apply(host: hostId, lines: [try runChanged("proj-a")])
        XCTAssertEqual(recorder.refreshes, [["proj-a"]])
        XCTAssertTrue(recorder.urgentRefreshes.isEmpty)
    }

    // MARK: Removing a project from the list

    private func twoProjects(second: String = "silent") throws -> [StreamLine] {
        var lines: [StreamLine] = [try StoreWire.hello()]
        lines.append(contentsOf: try runProject("proj-a"))
        lines.append(contentsOf: try runProject("proj-b"))
        lines.append(try StoreWire.project("proj-b", state: second))
        lines.append(try StoreWire.cursor(makeCursor(["proj-a", "proj-b"])))
        return lines
    }

    func testASilentProjectIsRemovedFromTheListTheFileAndTheCursor() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(host: hostId, lines: try twoProjects())
        let result = await engine.removeProject("proj-b")
        XCTAssertEqual(result, .removed)
        let model = await engine.current()
        XCTAssertNil(model.projects["proj-b"])
        XCTAssertNotNil(model.projects["proj-a"])
        XCTAssertEqual(Set(try XCTUnwrap(model.cursor).decoded().p.keys), ["proj-a"], "the cursor no longer carries it")
        let saved = try XCTUnwrap(store.stored)
        XCTAssertNil(saved.projects["proj-b"])
        XCTAssertEqual(Set(try XCTUnwrap(saved.cursor).decoded().p.keys), ["proj-a"])
        let resume = await engine.resumeCursor(host: hostId)
        XCTAssertEqual(Set(try XCTUnwrap(resume).decoded().p.keys), ["proj-a"])
    }

    func testAGoneProjectCanBeRemoved() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        await engine.apply(host: hostId, lines: try twoProjects(second: "gone"))
        let result = await engine.removeProject("proj-b")
        XCTAssertEqual(result, .removed)
    }

    func testALiveProjectIsRefusedAndKept() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(host: hostId, lines: try twoProjects())
        let result = await engine.removeProject("proj-a")
        XCTAssertEqual(result, .refusedLive)
        let model = await engine.current()
        XCTAssertNotNil(model.projects["proj-a"])
        XCTAssertEqual(Set(try XCTUnwrap(model.cursor).decoded().p.keys), ["proj-a", "proj-b"])
        let missing = await engine.removeProject("proj-z")
        XCTAssertEqual(missing, .notFound)
    }

    func testTheStreamCursorLineDoesNotBringTheRemovedProjectBack() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        await engine.apply(host: hostId, lines: try twoProjects())
        await engine.removeProject("proj-b")
        await engine.apply(host: hostId, lines: [try StoreWire.cursor(makeCursor(["proj-a", "proj-b"]))])
        let kept = await engine.current().cursor
        XCTAssertEqual(Set(try XCTUnwrap(kept).decoded().p.keys), ["proj-a"])
    }

    func testRediscoveringTheControllerBringsTheProjectBack() async throws {
        let hostId = UUID()
        let engine = makeEngine(hostId: hostId, persistence: MemoryPersistence())
        await engine.apply(host: hostId, lines: try twoProjects())
        await engine.removeProject("proj-b")
        var again: [StreamLine] = try runProject("proj-b", hash: "run-hash-9")
        again.append(try StoreWire.cursor(makeCursor(["proj-a", "proj-b"])))
        await engine.apply(host: hostId, lines: again)
        let model = await engine.current()
        XCTAssertEqual(model.projects["proj-b"]?.availability, .ready)
        XCTAssertEqual(model.projects["proj-b"]?.run?.contentHash, "run-hash-9")
        XCTAssertEqual(Set(try XCTUnwrap(model.cursor).decoded().p.keys), ["proj-a", "proj-b"])
    }

    func testOnlyTheTwoFreshnessTimesAreSavedAndAnOlderStoreDecodesWithNoMarks() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        await engine.apply(host: hostId, lines: try [StoreWire.hello()] + completeProject())
        await engine.flush()
        let saved = try XCTUnwrap(store.stored)
        let marks = try XCTUnwrap(saved.projects["proj-a"]?.freshness)
        XCTAssertNotNil(marks.lastKnownAt)
        XCTAssertNil(marks.refreshFailedAt)
        XCTAssertNil(marks.currentAsOf)
        XCTAssertFalse(marks.refreshing)
        XCTAssertTrue(marks.pending.isEmpty)

        let data = try JSONEncoder().encode(saved)
        var tree = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        var projects = try XCTUnwrap(tree["projects"] as? [String: Any])
        var project = try XCTUnwrap(projects["proj-a"] as? [String: Any])
        project["freshness"] = nil
        projects["proj-a"] = project
        tree["projects"] = projects
        let older = try JSONDecoder().decode(
            PersistedHost.self, from: try JSONSerialization.data(withJSONObject: tree))
        XCTAssertNil(older.projects["proj-a"]?.freshness)
        XCTAssertNil(older.projects["proj-a"]?.foldedSections)
    }

    func testFoldedSectionsAreKeptPerProjectAcrossAReloadAndUnfold() async throws {
        let hostId = UUID()
        let store = MemoryPersistence()
        let engine = makeEngine(hostId: hostId, persistence: store)
        let lines = try [StoreWire.hello()] + completeProject("proj-a") + completeProject("proj-b")
        await engine.apply(host: hostId, lines: lines)
        await engine.setFolded(projectId: "proj-a", ["plans", "outcomes"])
        let reloaded = makeEngine(hostId: hostId, persistence: MemoryPersistence(initial: store.stored))
        let model = await reloaded.current()
        XCTAssertEqual(model.projects["proj-a"]?.folded, ["plans", "outcomes"])
        XCTAssertEqual(model.projects["proj-b"]?.folded, [])

        let second = MemoryPersistence(initial: store.stored)
        let again = makeEngine(hostId: hostId, persistence: second)
        await again.setFolded(projectId: "proj-a", [])
        let last = makeEngine(hostId: hostId, persistence: MemoryPersistence(initial: second.stored))
        let unfolded = await last.current()
        XCTAssertEqual(unfolded.projects["proj-a"]?.folded, [])
        XCTAssertNil(unfolded.projects["proj-a"]?.foldedSections)
    }
}
