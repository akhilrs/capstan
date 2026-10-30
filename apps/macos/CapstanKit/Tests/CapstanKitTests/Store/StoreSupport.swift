import Foundation
import XCTest
@testable import CapstanKit

/// Lines and models for the store tests.
enum StoreWire {
    static let since = "2026-10-01T09:30:00.000Z"

    static func line(_ json: String) throws -> StreamLine {
        try LineDecoder().decode(Data(json.utf8))
    }

    static func hello(projects: Int = 2) throws -> StreamLine {
        try line("{\"v\":1,\"kind\":\"hello\",\"version\":\"0.1.0\",\"host\":\"build-host\",\"projects\":\(projects)}")
    }

    static func project(_ id: String, state: String = "ready", head: Int = 26) throws -> StreamLine {
        let ready = state == "ready"
        return try line(
            "{\"v\":1,\"kind\":\"project\",\"projectId\":\"\(id)\",\"root\":\"/srv/work/\(id)\",\"state\":\"\(state)\","
                + "\"epoch\":\(ready ? "\"daeca3b2245f3519\"" : "null"),\"head\":\(ready ? String(head) : "null")}")
    }

    static func snapshot(_ project: String, _ section: String, key: String, item: String) throws -> StreamLine {
        try line(
            "{\"v\":1,\"kind\":\"snapshot\",\"projectId\":\"\(project)\",\"section\":\"\(section)\",\"key\":\"\(key)\","
                + "\"item\":\(item)}")
    }

    static func end(_ project: String, head: Int = 26) throws -> StreamLine {
        try line("{\"v\":1,\"kind\":\"snapshotEnd\",\"projectId\":\"\(project)\",\"head\":\(head)}")
    }

    static func resync(_ project: String?) throws -> StreamLine {
        let id = project.map { "\"\($0)\"" } ?? "null"
        return try line("{\"v\":1,\"kind\":\"resync\",\"projectId\":\(id),\"reason\":\"epoch\"}")
    }

    static func cursor(_ cursor: Cursor) throws -> StreamLine {
        try line("{\"v\":1,\"kind\":\"cursor\",\"cursor\":\"\(cursor.text)\"}")
    }

    static func waitingItem(_ key: String, summary: String = "run ls", hash: String = "hash-1", since: String = since) -> String {
        let kind = String(key.split(separator: ":").first ?? "")
        return "{\"key\":\"\(key)\",\"requestId\":\"req-\(key)\",\"kind\":\"\(kind)\",\"since\":\"\(since)\","
            + "\"summary\":\"\(summary)\",\"contentHash\":\"\(hash)\",\"needsTerminal\":false}"
    }

    static func runItem(state: String = "active", paused: Bool = false, hash: String = "run-hash-1") -> String {
        "{\"requestId\":\"run\",\"state\":\"\(state)\",\"paused\":\(paused),\"contentHash\":\"\(hash)\"}"
    }

    /// The `run.changed` event as the host sends it: the new state in the body, and an invalidate as its only effect.
    static func runChanged(state: String, paused: Bool, seq: Int = 31) throws -> StreamLine {
        .event(projectId: "proj-a", try event(
            "run.changed", entity: ("run_control", "proj-a"), seq: seq, effects: [("run", "invalidate", "run", "{}")],
            extraBody: ",\"state\":\"\(state)\",\"paused\":\(paused),\"reason\":\"maintenance\","
                + "\"textVerified\":true,\"truncated\":false,\"untrusted\":[\"body.reason\"]"))
    }

    static func agentItem(_ id: String, state: String = "active", paused: Bool = false, task: String? = nil) -> String {
        let taskText = task.map { "\"\($0)\"" } ?? "null"
        return "{\"key\":\"\(id)\",\"agentId\":\"\(id)\",\"kind\":\"Developer\",\"role\":\"developer\",\"generation\":1,"
            + "\"state\":\"\(state)\",\"paused\":\(paused),\"task\":\(taskText),\"requestId\":\"agent:\(id)\","
            + "\"contentHash\":\"agent-hash\"}"
    }

    static func planItem(
        _ key: String = "1", total: Int = 3, assigned: Int = 2, reported: Int = 1, integrated: Int = 0
    ) -> String {
        "{\"key\":\"\(key)\",\"planId\":\"plan-\(key)\",\"sequence\":\(key),\"title\":\"Remote protocol\","
            + "\"state\":\"approved\",\"packages\":{\"total\":\(total),\"assigned\":\(assigned),"
            + "\"reported\":\(reported),\"integrated\":\(integrated)}}"
    }

    /// An event built from effects, each `(section, op, key, fields JSON)`.
    static func event(
        _ type: String, entity: (String, String) = ("agent", "dev-1"), seq: Int = 30,
        effects: [(String, String, String, String)], extraBody: String = ""
    ) throws -> FeedEvent {
        let list = effects.map { "{\"section\":\"\($0.0)\",\"op\":\"\($0.1)\",\"key\":\"\($0.2)\",\"fields\":\($0.3)}" }
        let json = "{\"id\":\"proj:epoch:\(seq)\",\"seq\":\(seq),\"k\":0,\"ts\":\"\(since)\",\"type\":\"\(type)\","
            + "\"actorId\":null,\"entity\":{\"type\":\"\(entity.0)\",\"id\":\"\(entity.1)\"},\"from\":null,\"to\":null,"
            + "\"body\":{\"effects\":[\(list.joined(separator: ","))]\(extraBody)}}"
        return try FeedEvent(json: try decodeJSON(Data(json.utf8)))
    }
}

/// Applies lines to one host model the way the engine does, without a clock.
struct ReducerHarness {
    var host = HostModel(hostId: UUID())
    var staging = Staging()

    @discardableResult
    mutating func apply(_ lines: StreamLine..., now: Date = Date(timeIntervalSince1970: 1_000_000)) -> ReduceOutput {
        apply(lines, now: now)
    }

    @discardableResult
    mutating func apply(_ lines: [StreamLine], now: Date = Date(timeIntervalSince1970: 1_000_000)) -> ReduceOutput {
        var total = ReduceOutput()
        for line in lines {
            let output = StoreReducer.apply(line, to: &host, staging: &staging, now: now)
            total.refresh.formUnion(output.refresh)
            total.urgent.formUnion(output.urgent)
            total.notify.append(contentsOf: output.notify)
        }
        return total
    }

    var alpha: ProjectModel? { host.projects["proj-a"] }
}

/// A persistence that keeps what it is given in memory.
final class MemoryPersistence: PersistenceStore, @unchecked Sendable {
    private let lock = NSLock()
    private var kept: PersistedHost?
    private var saved: [PersistedHost] = []
    private var deleted = 0

    init(initial: PersistedHost? = nil) {
        kept = initial
    }

    func load(hostId: UUID) -> PersistedHost? {
        lock.withLock { kept }
    }

    func save(_ host: PersistedHost) throws {
        lock.withLock {
            kept = host
            saved.append(host)
        }
    }

    func delete(hostId: UUID) throws {
        lock.withLock {
            kept = nil
            deleted += 1
        }
    }

    var saves: [PersistedHost] { lock.withLock { saved } }
    var deletes: Int { lock.withLock { deleted } }
    var stored: PersistedHost? { lock.withLock { kept } }
}

/// Collects what an engine reports.
final class EngineRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var models: [HostModel] = []
    private var notified: [[NotifyCandidate]] = []
    private var refreshed: [Set<String>] = []
    private var urgentRefreshed: [Set<String>] = []

    func change(_ model: HostModel) { lock.withLock { models.append(model) } }
    func notify(_ list: [NotifyCandidate]) { lock.withLock { notified.append(list) } }
    func refresh(_ ids: Set<String>) { lock.withLock { refreshed.append(ids) } }
    func urgentRefresh(_ ids: Set<String>) { lock.withLock { urgentRefreshed.append(ids) } }

    var notifications: [NotifyCandidate] { lock.withLock { notified.flatMap { $0 } } }
    var refreshes: [Set<String>] { lock.withLock { refreshed } }
    var urgentRefreshes: [Set<String>] { lock.withLock { urgentRefreshed } }
    var changes: Int { lock.withLock { models.count } }
}

func makeEngine(
    hostId: UUID, persistence: any PersistenceStore, clock: any SessionClock = SystemClock(),
    recorder: EngineRecorder = EngineRecorder(), urgentCallback: Bool = false
) -> HostStoreEngine {
    HostStoreEngine(
        hostId: hostId, persistence: persistence, clock: clock,
        onChange: { recorder.change($0) }, onNotify: { recorder.notify($0) }, onRefresh: { recorder.refresh($0) },
        onUrgentRefresh: urgentCallback ? { recorder.urgentRefresh($0) } : nil)
}

/// The first item of the page example of the protocol document that holds `marker`, as JSON text.
func docItem(_ marker: String) throws -> String {
    let block = try XCTUnwrap(ProtocolDoc.jsonBlocks().first { $0.text.contains(marker) }, marker)
    let page = try decodeJSON(Data(block.text.utf8))
    let item = try XCTUnwrap(page["items"]?[0])
    return String(decoding: try JSONEncoder().encode(item), as: UTF8.self)
}

func docStreamStart() throws -> [StreamLine] {
    let block = try XCTUnwrap(ProtocolDoc.jsonBlocks().first { $0.text.contains("\"kind\":\"hello\"") })
    return try block.text.split(separator: "\n").map { try StoreWire.line(String($0)) }
}
