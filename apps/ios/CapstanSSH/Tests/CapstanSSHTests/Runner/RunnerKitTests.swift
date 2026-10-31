import CapstanKit
import Foundation
import XCTest
@testable import CapstanSSH

/// CapstanKit's `HostSession`, `checkHost` and `Decider` run unchanged on `SSHRunner`, against a fake `cstan`.
final class RunnerKitTests: XCTestCase {
    private var fixture: RunnerFixture!

    private static var cursor: Cursor {
        Cursor(payload: CursorPayload(p: [
            "p1": CursorEntry(e: "0123456789abcdef", s: 3, h: "fedcba9876543210", r: "/work/p1"),
        ]))
    }

    /// `cstan events` prints a protocol v1 snapshot and waits; `help` knows events and decide; `decide` hangs.
    private static var fakeCstan: String {
        let lines = [
            #"{"v":1,"kind":"hello","version":"0.1","host":"box","projects":1}"#,
            #"{"v":1,"kind":"project","projectId":"p1","root":"/work/p1","state":"ready"}"#,
            #"{"v":1,"kind":"snapshotEnd","projectId":"p1","head":3}"#,
            #"{"v":1,"kind":"cursor","cursor":"\#(cursor.text)"}"#,
        ]
        let printed = lines.map { "printf '%s\\n' '\($0)'" }.joined(separator: "\n    ")
        return """
            case "$1" in
              events)
                \(printed)
                sleep 30 ;;
              help)
                case "$2" in events|decide) exit 0 ;; *) exit 2 ;; esac ;;
              decide)
                sleep 30 ;;
              *)
                exit 2 ;;
            esac
            """
    }

    override func setUpWithError() throws {
        fixture = try RunnerFixture()
        try fixture.server.installScript("cstan", RunnerKitTests.fakeCstan)
    }

    override func tearDown() {
        fixture.stop()
        fixture = nil
    }

    func testHostSessionReadsASnapshot() async throws {
        let consumer = Consumer()
        let session = HostSession(config: fixture.config, runner: fixture.runner, consumer: consumer, clock: fixture.clock)
        await session.start()
        await eventually(timeout: 10, "a batch") { await consumer.batchCount >= 1 }
        let batch = await consumer.firstBatch
        XCTAssertGreaterThanOrEqual(batch.count, 2, "the snapshot lines and the cursor")
        if case .cursor(let cursor)? = batch.last {
            XCTAssertEqual(cursor.text, RunnerKitTests.cursor.text)
        } else {
            XCTFail("the batch ends at a cursor line")
        }
        await session.stop()
    }

    func testCheckHostPassesEveryStep() async throws {
        var checks: [HostCheck] = []
        for await check in checkHost(fixture.config, fixture.runner, clock: fixture.clock) { checks.append(check) }
        XCTAssertEqual(checks.map(\.step), [.connect, .capstan, .projects, .firstSnapshot])
        XCTAssertEqual(checks.map(\.status), [.passed, .passed, .passed, .passed], "\(checks.map(\.text))")
    }

    func testSuspendDuringADecisionKeepsItForRetry() async throws {
        let pendingURL = fixture.directory.appendingPathComponent("pending.json")
        let pending = PendingStore(url: pendingURL)
        let decider = Decider(runner: fixture.runner, pending: pending, clock: fixture.clock)
        let request = DecideRequest(
            hostId: fixture.host.id, projectId: "p1", requestId: "op:p1", expect: "0123456789ab", client: "iPhone",
            verb: .opDeny(note: nil))
        let config = fixture.config
        let sending = Task { await decider.send(request, config: config) }
        await eventually(timeout: 5, "the decide call") {
            self.fixture.server.log.execs.contains { $0.contains("decide") }
        }
        fixture.runner.suspend()
        let outcome = await sending.value
        guard case .notSent(reason: .ssh, let pendingId) = outcome else {
            return XCTFail("expected notSent with an ssh reason, got \(outcome)")
        }
        let kept = await pending.record(pendingId)
        XCTAssertEqual(kept?.request, request, "the request stays for retry")
    }
}

private actor Consumer: StreamConsumer {
    private var batches: [[StreamLine]] = []

    var batchCount: Int { batches.count }
    var firstBatch: [StreamLine] { batches.first ?? [] }

    func apply(host: UUID, lines: [StreamLine]) async { batches.append(lines) }
    func connection(host: UUID, _ state: ConnectionState) async {}
    func resumeCursor(host: UUID) async -> Cursor? { nil }
    func dropCursor(host: UUID) async {}
}
