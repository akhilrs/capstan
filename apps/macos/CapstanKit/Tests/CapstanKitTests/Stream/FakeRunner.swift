import Foundation
import XCTest
@testable import CapstanKit

/// What a fake child does as soon as it starts.
struct FakeScript {
    var chunks: [String] = []
    /// Nil keeps the child running until the test ends it.
    var exit: Int32?
    var stderr = ""
    var stdout = ""
    /// A terminate ends the output at once but the child only exits when the test calls `end`.
    var slowExit = false

    init(
        chunks: [String] = [], exit: Int32? = nil, stderr: String = "", stdout: String = "", slowExit: Bool = false
    ) {
        self.slowExit = slowExit
        self.chunks = chunks
        self.exit = exit
        self.stderr = stderr
        self.stdout = stdout
    }
}

final class FakeProcess: RunningProcess, @unchecked Sendable {
    let lines: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let lock = NSLock()
    private var splitter = LineSplitter()
    private var stdoutData = Data()
    private var stderrText = ""
    private var terminations = 0
    private let exit = Latch<Int32>()
    private let slowExit: Bool
    private var exited = false

    init(script: FakeScript) {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        lines = stream
        self.continuation = continuation
        slowExit = script.slowExit
        stdoutData = Data(script.stdout.utf8)
        stderrText = script.stderr
        for chunk in script.chunks { send(chunk) }
        if let code = script.exit { end(exit: code) }
    }

    var isRunning: Bool {
        lock.withLock { !exited }
    }

    var terminateCount: Int {
        lock.withLock { terminations }
    }

    /// Standard output as the child wrote it, in any chunking.
    func send(_ chunk: String) {
        send(Data(chunk.utf8))
    }

    func send(_ chunk: Data) {
        let result: Result<[Data], Error> = lock.withLock {
            do { return .success(try splitter.feed(chunk)) } catch { return .failure(error) }
        }
        switch result {
        case .success(let all): for line in all { continuation.yield(line) }
        case .failure(let error): continuation.finish(throwing: error)
        }
    }

    func end(exit code: Int32, stderr: String? = nil) {
        if let stderr { lock.withLock { stderrText = stderr } }
        continuation.finish()
        lock.withLock { exited = true }
        exit.set(code)
    }

    func stdoutAll() async -> Data {
        _ = await exit.wait()
        return lock.withLock { stdoutData }
    }

    func stderrTail() -> String {
        lock.withLock { stderrText }
    }

    func terminate(grace: TimeInterval) {
        lock.withLock { terminations += 1 }
        continuation.finish()
        if slowExit { return }
        lock.withLock { exited = true }
        exit.set(143)
    }

    func exitCode() async -> Int32 {
        await exit.wait()
    }
}

struct FakeStartError: Error {}

/// Hands out fake children, one script per start; a start with no script gets a child that stays silent.
final class FakeRunner: ProcessRunner, @unchecked Sendable {
    private let lock = NSLock()
    private var scripts: [FakeScript] = []
    private var started: [(executable: URL, arguments: [String])] = []
    private var children: [FakeProcess] = []
    private var failNext = false
    private var maxConcurrent = 0

    func enqueue(_ script: FakeScript) {
        lock.withLock { scripts.append(script) }
    }

    func failNextStart() {
        lock.withLock { failNext = true }
    }

    func start(_ executable: URL, _ arguments: [String]) throws -> any RunningProcess {
        let (script, fail): (FakeScript, Bool) = lock.withLock {
            let fail = failNext
            failNext = false
            return (scripts.isEmpty ? FakeScript() : scripts.removeFirst(), fail)
        }
        if fail { throw FakeStartError() }
        let child = FakeProcess(script: script)
        lock.withLock {
            let live = children.filter { $0.isRunning }.count + 1
            maxConcurrent = max(maxConcurrent, live)
            started.append((executable, arguments))
            children.append(child)
        }
        return child
    }

    /// The most children that were running at the same moment.
    var maxConcurrentChildren: Int {
        lock.withLock { maxConcurrent }
    }

    var startCount: Int {
        lock.withLock { started.count }
    }

    func arguments(at index: Int) -> [String] {
        lock.withLock { started[index].arguments }
    }

    func child(at index: Int) -> FakeProcess {
        lock.withLock { children[index] }
    }

    /// The remote command line of a start, split back into words the way the host's shell would.
    func remoteWords(at index: Int) throws -> [String] {
        guard let remote = arguments(at: index).last else { return [] }
        return try ShParser.split(remote)
    }
}

/// A consumer that records everything and can be told what cursor to resume from.
actor RecordingConsumer: StreamConsumer {
    private(set) var batches: [[StreamLine]] = []
    private(set) var states: [ConnectionState] = []
    private(set) var dropCount = 0
    private var stored: Cursor?
    private var gate: Latch<Void>?

    init(cursor: Cursor? = nil) {
        stored = cursor
    }

    /// Makes `apply` wait until the latch is set.
    func hold(_ latch: Latch<Void>?) {
        gate = latch
    }

    func apply(host: UUID, lines: [StreamLine]) async {
        if let gate { await gate.wait() }
        batches.append(lines)
    }

    func connection(host: UUID, _ state: ConnectionState) async {
        states.append(state)
    }

    func resumeCursor(host: UUID) async -> Cursor? {
        stored
    }

    func dropCursor(host: UUID) async {
        dropCount += 1
        stored = nil
    }

    func lastState() -> ConnectionState? {
        states.last
    }
}
