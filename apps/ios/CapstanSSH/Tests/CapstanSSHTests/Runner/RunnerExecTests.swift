import CapstanKit
import Foundation
import XCTest
@testable import CapstanSSH

/// Exec channels behave as the Mac's `LiveProcess`.
final class RunnerExecTests: XCTestCase {
    private var fixture: RunnerFixture!

    override func setUpWithError() throws {
        fixture = try RunnerFixture()
    }

    override func tearDown() {
        fixture.stop()
        fixture = nil
    }

    func testEchoGivesTheLineAndExitZero() async throws {
        let process = try fixture.start("echo hello")
        let result = await collect(process)
        XCTAssertEqual(result.lines, ["hello"])
        XCTAssertNil(result.error)
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
    }

    func testExitCodeIsTheRemotes() async throws {
        let process = try fixture.start("exit 3")
        let code = await exitCode(process)
        XCTAssertEqual(code, 3)
    }

    func testALineOverTheCapEndsTheLinesWithTooLong() async throws {
        let process = try fixture.start("head -c 70000 /dev/zero | tr '\\000' a; echo")
        let result = await collect(process)
        XCTAssertEqual(result.error as? LineDecodeError, .tooLong)
        _ = await exitCode(process)
    }

    func testStdoutAllKeepsOneMebibyte() async throws {
        let process = try fixture.start("head -c 2000000 /dev/zero | tr '\\000' a | fold -w 99")
        let all = await process.stdoutAll()
        XCTAssertEqual(all.count, 1_048_576)
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
    }

    func testStderrTailIsTheLast4096BytesAsLossyUTF8() async throws {
        let process = try fixture.start("head -c 5000 /dev/zero | tr '\\000' e >&2; printf '\\377' >&2")
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
        let tail = process.stderrTail()
        XCTAssertEqual(tail, String(repeating: "e", count: 4095) + "\u{FFFD}")
    }

    func testStdinIsClosedSoCatEnds() async throws {
        let process = try fixture.start("cat")
        let result = await collect(process)
        XCTAssertEqual(result.lines, [])
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
        let all = await process.stdoutAll()
        XCTAssertTrue(all.isEmpty)
    }

    func testExitSignalIs128PlusTheSignal() async throws {
        let process = try fixture.start("kill -TERM $$")
        let code = await exitCode(process)
        XCTAssertEqual(code, 128 + 15)
    }

    func testLinesAndStderrTogether() async throws {
        let process = try fixture.start("echo one; echo oops >&2; echo two")
        let result = await collect(process)
        XCTAssertEqual(result.lines, ["one", "two"])
        let code = await exitCode(process)
        XCTAssertEqual(code, 0)
        XCTAssertEqual(process.stderrTail(), "oops\n")
    }
}
