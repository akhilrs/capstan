import Foundation

/// Starts a child process. The live runner uses Foundation; tests use a fake.
public protocol ProcessRunner: Sendable {
    func start(_ executable: URL, _ arguments: [String]) throws -> any RunningProcess
}

public protocol RunningProcess: Sendable {
    /// Standard output split on newline, without the newline. A line over 65535 bytes ends the stream with
    /// `LineDecodeError.tooLong`.
    var lines: AsyncThrowingStream<Data, Error> { get }
    /// All of standard output, once the child closed it.
    func stdoutAll() async -> Data
    /// The last 4096 bytes of standard error as lossy UTF-8. Host text for a person only.
    func stderrTail() -> String
    /// SIGTERM to this child, SIGKILL after the grace period if it is still running.
    func terminate(grace: TimeInterval)
    /// The exit code once the child ended; 128 plus the signal number when a signal ended it.
    func exitCode() async -> Int32
}

extension RunningProcess {
    public func terminate() {
        terminate(grace: 3)
    }
}

/// Cuts a byte stream into lines.
struct LineSplitter {
    /// The longest line without its newline.
    static let maxLineBytes = LineDecoder.maxLineBytes - 1

    private var buffer = Data()

    /// The complete lines in the chunk, in order. Throws `LineDecodeError.tooLong` when a line passes the cap.
    mutating func feed(_ chunk: Data) throws -> [Data] {
        buffer.append(chunk)
        var lines: [Data] = []
        var start = buffer.startIndex
        while let newline = buffer[start...].firstIndex(of: 0x0A) {
            if newline - start > LineSplitter.maxLineBytes { throw LineDecodeError.tooLong }
            lines.append(Data(buffer[start..<newline]))
            start = buffer.index(after: newline)
        }
        // Compact once per chunk, not once per line.
        buffer = Data(buffer[start...])
        if buffer.count > LineSplitter.maxLineBytes { throw LineDecodeError.tooLong }
        return lines
    }
}

/// A value set once that any number of tasks can wait for.
final class Latch<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    /// Sets the value; later calls change nothing.
    func set(_ newValue: Value) {
        let resumed: [CheckedContinuation<Value, Never>] = lock.withLock {
            guard value == nil else { return [] }
            value = newValue
            let pending = waiters
            waiters = []
            return pending
        }
        for waiter in resumed { waiter.resume(returning: newValue) }
    }

    func wait() async -> Value {
        await withCheckedContinuation { (continuation: CheckedContinuation<Value, Never>) in
            let ready: Value? = lock.withLock {
                if let value { return value }
                waiters.append(continuation)
                return nil
            }
            if let ready { continuation.resume(returning: ready) }
        }
    }
}

public struct LiveProcessRunner: ProcessRunner {
    public init() {}

    public func start(_ executable: URL, _ arguments: [String]) throws -> any RunningProcess {
        try LiveProcess(executable: executable, arguments: arguments)
    }
}

final class LiveProcess: RunningProcess, @unchecked Sendable {
    /// Standard output kept for `stdoutAll`; a stream read through `lines` is long and not kept past this.
    private static let keepCap = 1_048_576
    private static let stderrCap = 4096

    let lines: AsyncThrowingStream<Data, Error>

    private let process = Process()
    private let lock = NSLock()
    private var kept = Data()
    private var errorTail = Data()
    private let outputDone = Latch<Void>()
    private let exit = Latch<Int32>()
    private let errorDone = DispatchSemaphore(value: 0)

    init(executable: URL, arguments: [String]) throws {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        lines = stream
        let outPipe = Pipe()
        let errPipe = Pipe()
        process.executableURL = executable
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = outPipe
        process.standardError = errPipe

        let outputDone = self.outputDone
        let exit = self.exit
        let errorDone = self.errorDone
        process.terminationHandler = { finished in
            // Give the reader of standard error a moment: a helper the child left behind may hold the pipe open.
            _ = errorDone.wait(timeout: .now() + 0.25)
            let status = finished.terminationStatus
            exit.set(finished.terminationReason == .uncaughtSignal ? 128 + status : status)
        }
        try process.run()

        let outHandle = outPipe.fileHandleForReading
        let errHandle = errPipe.fileHandleForReading
        Thread.detachNewThread { [self] in
            readOutput(outHandle, continuation)
            outputDone.set(())
        }
        Thread.detachNewThread { [self] in
            readError(errHandle)
            errorDone.signal()
        }
    }

    private func readOutput(_ handle: FileHandle, _ continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        var splitter = LineSplitter()
        while true {
            let chunk: Data
            do {
                chunk = try handle.read(upToCount: 65536) ?? Data()
            } catch {
                break
            }
            if chunk.isEmpty { break }
            lock.withLock {
                if kept.count < LiveProcess.keepCap { kept.append(chunk.prefix(LiveProcess.keepCap - kept.count)) }
            }
            do {
                for line in try splitter.feed(chunk) { continuation.yield(line) }
            } catch {
                continuation.finish(throwing: error)
                terminate(grace: 3)
                drain(handle)
                return
            }
        }
        continuation.finish()
    }

    /// Reads and drops the rest so the child is never blocked on a full pipe.
    private func drain(_ handle: FileHandle) {
        while let chunk = try? handle.read(upToCount: 65536), !chunk.isEmpty {}
    }

    private func readError(_ handle: FileHandle) {
        while let chunk = try? handle.read(upToCount: 65536), !chunk.isEmpty {
            lock.withLock {
                errorTail.append(chunk)
                if errorTail.count > LiveProcess.stderrCap {
                    errorTail = Data(errorTail.suffix(LiveProcess.stderrCap))
                }
            }
        }
    }

    func stdoutAll() async -> Data {
        await outputDone.wait()
        return lock.withLock { kept }
    }

    func stderrTail() -> String {
        let tail = lock.withLock { errorTail }
        return String(decoding: tail, as: UTF8.self)
    }

    func terminate(grace: TimeInterval) {
        guard process.isRunning else { return }
        process.terminate()
        let pid = process.processIdentifier
        let child = self.process
        DispatchQueue.global().asyncAfter(deadline: .now() + max(grace, 0)) {
            if child.isRunning { kill(pid, SIGKILL) }
        }
    }

    func exitCode() async -> Int32 {
        await exit.wait()
    }
}
