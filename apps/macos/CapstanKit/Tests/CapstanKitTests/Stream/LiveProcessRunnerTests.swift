import XCTest
@testable import CapstanKit

/// These start real children. They are unverified until run on a Mac.
final class LiveProcessRunnerTests: XCTestCase {
    private let sh = URL(fileURLWithPath: "/bin/sh")

    func testLinesWithASplitWrite() async throws {
        let process = try LiveProcessRunner().start(sh, ["-c", "printf 'one\\ntw'; sleep 0.2; printf 'o\\nthree\\n'"])
        var lines: [String] = []
        for try await line in process.lines { lines.append(String(decoding: line, as: UTF8.self)) }
        XCTAssertEqual(lines, ["one", "two", "three"])
        let code = await process.exitCode()
        XCTAssertEqual(code, 0)
    }

    func testStdoutAllAndStderrTail() async throws {
        let process = try LiveProcessRunner().start(sh, ["-c", "echo out; echo problem >&2; exit 3"])
        let output = await process.stdoutAll()
        XCTAssertEqual(String(decoding: output, as: UTF8.self), "out\n")
        let code = await process.exitCode()
        XCTAssertEqual(code, 3)
        XCTAssertEqual(process.stderrTail(), "problem\n")
    }

    func testStderrTailKeepsTheLast4096Bytes() async throws {
        let process = try LiveProcessRunner().start(
            sh, ["-c", "i=0; while [ $i -lt 600 ]; do echo 0123456789; i=$((i+1)); done >&2; echo last >&2"])
        _ = await process.exitCode()
        let tail = process.stderrTail()
        XCTAssertLessThanOrEqual(tail.utf8.count, 4096)
        XCTAssertTrue(tail.hasSuffix("last\n"))
    }

    func testTooLongLineEndsTheStream() async throws {
        let process = try LiveProcessRunner().start(sh, ["-c", "head -c 70000 /dev/zero | tr '\\0' a; echo"])
        do {
            for try await _ in process.lines {}
            XCTFail("expected tooLong")
        } catch {
            XCTAssertEqual(error as? LineDecodeError, .tooLong)
        }
    }

    func testTerminateKillsAChildThatIgnoresTerm() async throws {
        let process = try LiveProcessRunner().start(sh, ["-c", "trap '' TERM; echo ready; while :; do :; done"])
        var iterator = process.lines.makeAsyncIterator()
        let first = try await iterator.next()
        XCTAssertEqual(first.map { String(decoding: $0, as: UTF8.self) }, "ready")
        let started = Date()
        process.terminate(grace: 0.2)
        let code = await process.exitCode()
        XCTAssertEqual(code, 128 + SIGKILL)
        XCTAssertGreaterThanOrEqual(Date().timeIntervalSince(started), 0.15)
        XCTAssertLessThan(Date().timeIntervalSince(started), 5)
    }

    func testTerminateEndsAWellBehavedChild() async throws {
        let process = try LiveProcessRunner().start(sh, ["-c", "echo ready; while :; do :; done"])
        var iterator = process.lines.makeAsyncIterator()
        _ = try await iterator.next()
        process.terminate(grace: 5)
        let code = await process.exitCode()
        XCTAssertEqual(code, 128 + SIGTERM)
    }
}
