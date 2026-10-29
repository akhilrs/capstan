import XCTest
@testable import CapstanKit

/// These start real children. Every test body runs on a detached task and is bounded by an XCTest expectation, so a
/// child or a reader that never finishes fails the test after `limit` seconds instead of hanging the run. The
/// timeout is XCTest's, not the cooperative pool's, so nothing the code under test blocks can hold it up.
final class LiveProcessRunnerTests: XCTestCase {
    private let sh = URL(fileURLWithPath: "/bin/sh")
    private let tracker = Tracker()

    /// Remembers every child so a test that fails or times out does not leave one running.
    private final class Tracker: @unchecked Sendable {
        private let lock = NSLock()
        private var children: [any RunningProcess] = []

        func add(_ child: any RunningProcess) { lock.withLock { children.append(child) } }
        func all() -> [any RunningProcess] { lock.withLock { children } }
    }

    override func tearDown() {
        // terminate returns at once: the signals are sent from a dispatch queue.
        for child in tracker.all() { child.terminate(grace: 0) }
        super.tearDown()
    }

    private func launch(_ script: String) throws -> any RunningProcess {
        let child = try LiveProcessRunner().start(sh, ["-c", script])
        tracker.add(child)
        return child
    }

    /// Runs `body` off the main executor and fails the test when it has not finished within `limit` seconds.
    private func bounded(
        _ name: String, limit: TimeInterval = 10, file: StaticString = #filePath, line: UInt = #line,
        _ body: @escaping @Sendable () async throws -> Void
    ) async {
        let done = expectation(description: name)
        let failure = Latch<String>()
        let finished = Latch<Bool>()
        let task = Task.detached {
            do { try await body() } catch { failure.set("\(error)") }
            finished.set(true)
            done.fulfill()
        }
        await fulfillment(of: [done], timeout: limit)
        if failure.peek() == nil, finished.peek() == nil { XCTFail("\(name) did not finish within \(limit) s", file: file, line: line) }
        if let message = failure.peek() { XCTFail(message, file: file, line: line) }
        task.cancel()
    }

    func testLinesWithASplitWrite() async throws {
        let process = try launch("printf 'one\\ntw'; sleep 0.2; printf 'o\\nthree\\n'")
        await bounded("split write") {
            var lines: [String] = []
            for try await line in process.lines { lines.append(String(decoding: line, as: UTF8.self)) }
            XCTAssertEqual(lines, ["one", "two", "three"])
            let code = await process.exitCode()
            XCTAssertEqual(code, 0)
        }
    }

    func testStdoutAllAndStderrTail() async throws {
        let process = try launch("echo out; echo problem >&2; exit 3")
        await bounded("stdout and stderr") {
            let output = await process.stdoutAll()
            XCTAssertEqual(String(decoding: output, as: UTF8.self), "out\n")
            let code = await process.exitCode()
            XCTAssertEqual(code, 3)
            XCTAssertEqual(process.stderrTail(), "problem\n")
        }
    }

    func testAFastChildsStderrIsWholeRightAfterItsExit() async throws {
        let children = try (0..<20).map { _ in try launch("echo 'cstan: unknown command: events' >&2; exit 2") }
        await bounded("fast children") {
            for process in children {
                let code = await process.exitCode()
                XCTAssertEqual(code, 2)
                XCTAssertEqual(process.stderrTail(), "cstan: unknown command: events\n")
            }
        }
    }

    /// A line must reach the reader as soon as the child flushes it, not when 64 KiB have collected.
    func testALineArrivesWhenTheChildWritesItNotWhenThePipeIsFull() async throws {
        let process = try launch("printf 'one\\n'; sleep 5; printf 'two\\n'")
        await bounded("first line") {
            var iterator = process.lines.makeAsyncIterator()
            let began = Date()
            let first = try await iterator.next()
            XCTAssertEqual(first.map { String(decoding: $0, as: UTF8.self) }, "one")
            XCTAssertLessThan(Date().timeIntervalSince(began), 3, "the first line waited for the second")
        }
    }

    /// A grandchild that keeps standard error open must not hold the exit back.
    func testAGrandchildHoldingStderrOpenDoesNotDelayTheExit() async throws {
        let process = try launch("sleep 6 & echo last words >&2; exit 4")
        await bounded("exit with open stderr") {
            let began = Date()
            let code = await process.exitCode()
            XCTAssertEqual(code, 4)
            XCTAssertEqual(process.stderrTail(), "last words\n")
            XCTAssertLessThan(Date().timeIntervalSince(began), 1.5, "the exit waited for the open pipe")
        }
    }

    /// Several writes and an immediate exit: every byte is in the tail when the exit is reported.
    func testEveryStderrByteIsKeptWhenTheChildExitsAtOnce() async throws {
        let children = try (0..<10).map { _ in
            try launch("i=0; while [ $i -lt 40 ]; do echo line-$i >&2; i=$((i+1)); done; echo end >&2; exit 1")
        }
        await bounded("immediate exit") {
            for process in children {
                let code = await process.exitCode()
                XCTAssertEqual(code, 1)
                let tail = process.stderrTail()
                XCTAssertTrue(tail.hasPrefix("line-0\n"), "tail: \(tail)")
                XCTAssertTrue(tail.hasSuffix("line-39\nend\n"), "tail: \(tail)")
            }
        }
    }

    func testStderrTailKeepsTheLast4096Bytes() async throws {
        let process = try launch("i=0; while [ $i -lt 600 ]; do echo 0123456789; i=$((i+1)); done >&2; echo last >&2")
        await bounded("stderr cap") {
            _ = await process.exitCode()
            let tail = process.stderrTail()
            XCTAssertLessThanOrEqual(tail.utf8.count, 4096)
            XCTAssertTrue(tail.hasSuffix("last\n"))
        }
    }

    func testTooLongLineEndsTheStream() async throws {
        let process = try launch("head -c 70000 /dev/zero | tr '\\0' a; echo")
        await bounded("too long") {
            do {
                for try await _ in process.lines {}
                XCTFail("expected tooLong")
            } catch {
                XCTAssertEqual(error as? LineDecodeError, .tooLong)
            }
        }
    }

    func testTerminateKillsAChildThatIgnoresTerm() async throws {
        let process = try launch("trap '' TERM; echo ready; while :; do :; done")
        await bounded("kill after grace") {
            var iterator = process.lines.makeAsyncIterator()
            let first = try await iterator.next()
            XCTAssertEqual(first.map { String(decoding: $0, as: UTF8.self) }, "ready")
            let began = Date()
            process.terminate(grace: 0.2)
            let code = await process.exitCode()
            XCTAssertEqual(code, 128 + SIGKILL)
            XCTAssertGreaterThanOrEqual(Date().timeIntervalSince(began), 0.15)
        }
    }

    func testTerminateEndsAWellBehavedChild() async throws {
        let process = try launch("echo ready; while :; do :; done")
        await bounded("terminate") {
            var iterator = process.lines.makeAsyncIterator()
            _ = try await iterator.next()
            process.terminate(grace: 5)
            let code = await process.exitCode()
            XCTAssertEqual(code, 128 + SIGTERM)
        }
    }
}
