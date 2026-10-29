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

    /// The value if it is set, without waiting.
    func peek() -> Value? {
        lock.withLock { value }
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

/// Reads a descriptor with read(2): whatever is available, at least one byte; nil at the end or on an error.
final class ChunkReader {
    private let descriptor: Int32
    private var buffer = [UInt8](repeating: 0, count: 65536)

    init(_ descriptor: Int32) {
        self.descriptor = descriptor
    }

    /// Blocks until the descriptor is readable (data, end of file or an error).
    func waitReadable() {
        var entry = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        while poll(&entry, 1, -1) < 0 && errno == EINTR {}
    }

    /// Whether the descriptor is readable now, without waiting.
    static func isReadable(_ descriptor: Int32) -> Bool {
        var entry = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        let result = poll(&entry, 1, 0)
        return result > 0 && entry.revents != 0
    }

    func next() -> Data? {
        while true {
            let count = buffer.withUnsafeMutableBytes { read(descriptor, $0.baseAddress, $0.count) }
            if count > 0 { return Data(buffer[0..<count]) }
            if count == 0 { return nil }
            if errno == EINTR { continue }
            return nil
        }
    }
}

final class LiveProcess: RunningProcess, @unchecked Sendable {
    /// Standard output kept for `stdoutAll`; a stream read through `lines` is long and not kept past this.
    private static let keepCap = 1_048_576
    private static let stderrCap = 4096
    /// How long after the exit the reader of standard error may take to reach its end before the exit is published
    /// anyway: only a helper the child left behind, holding the pipe open, makes it run out.
    private static let stderrGrace = 2.0

    let lines: AsyncThrowingStream<Data, Error>

    private let process = Process()
    private let lock = NSLock()
    private var kept = Data()
    private var errorTail = Data()
    private var finished = false
    private let outputDone = Latch<Void>()
    private let exit = Latch<Int32>()
    private var errorDrained = false
    private var exitPublished = false
    /// True while the reader of standard error is blocked in `read` with nothing taken yet; guarded by `lock`.
    private var errorReaderWaiting = false
    private var errorDescriptor: Int32 = -1

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
        process.terminationHandler = { [weak self] _ in self?.finish() }
        try process.run()

        let outHandle = outPipe.fileHandleForReading
        let errHandle = errPipe.fileHandleForReading
        errorDescriptor = errHandle.fileDescriptor
        Thread.detachNewThread { [self] in
            readOutput(outHandle, continuation)
            outputDone.set(())
        }
        Thread.detachNewThread { [self] in
            readError(errHandle)
            lock.withLock { errorDrained = true }
            publishExitIfReady()
        }
        // The termination handler is the first way to learn of the exit; this poll is the second, so a handler that
        // does not run cannot leave `exitCode` waiting for good.
        Thread.detachNewThread { [self] in
            while process.isRunning { usleep(20_000) }
            finish()
        }
    }

    /// Records the exit once, from whichever of the termination handler and the poll sees it first.
    private func finish() {
        let first: Bool = lock.withLock {
            guard !finished else { return false }
            finished = true
            return true
        }
        guard first else { return }
        publishExitIfReady()
        // The exit is published once standard error is drained: at its end, or when the reader is waiting with nothing
        // left in the pipe. A grandchild that keeps the pipe open therefore delays nothing, and a child's last words
        // are in `stderrTail` by then. A hard cap publishes it anyway.
        pollExit(deadline: Date().addingTimeInterval(LiveProcess.stderrGrace))
    }

    private func pollExit(deadline: Date) {
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.01) { [self] in
            // Drained: at its end, or the reader is waiting for the pipe to become readable and nothing is readable.
            // The reader clears `errorReaderWaiting` before it takes bytes and sets it again only after it has kept
            // them, so there is no moment with bytes taken, not kept and the flag set.
            let drained: Bool = lock.withLock {
                errorDrained || (errorReaderWaiting && !ChunkReader.isReadable(errorDescriptor))
            }
            if drained || Date() >= deadline {
                publishExit()
            } else {
                pollExit(deadline: deadline)
            }
        }
    }

    private func publishExitIfReady() {
        let ready: Bool = lock.withLock { finished && errorDrained }
        if ready { publishExit() }
    }

    private func publishExit() {
        let first: Bool = lock.withLock {
            guard !exitPublished else { return false }
            exitPublished = true
            return true
        }
        guard first else { return }
        let status = process.terminationStatus
        exit.set(process.terminationReason == .uncaughtSignal ? 128 + status : status)
    }

    private func readOutput(_ handle: FileHandle, _ continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        var splitter = LineSplitter()
        let reader = ChunkReader(handle.fileDescriptor)
        while true {
            // read(2) returns what is there; FileHandle.read(upToCount:) may wait for the whole count on a pipe and
            // hold a line back until 64 KiB have arrived.
            guard let chunk = reader.next(), !chunk.isEmpty else { break }
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
        let reader = ChunkReader(handle.fileDescriptor)
        while let chunk = reader.next(), !chunk.isEmpty {}
    }

    private func readError(_ handle: FileHandle) {
        let reader = ChunkReader(handle.fileDescriptor)
        while true {
            lock.withLock { errorReaderWaiting = true }
            reader.waitReadable()
            // Cleared before the bytes are taken, so an exit that polls now sees a reader that is not waiting.
            lock.withLock { errorReaderWaiting = false }
            guard let chunk = reader.next(), !chunk.isEmpty else { break }
            lock.withLock {
                errorTail.append(chunk)
                if errorTail.count > LiveProcess.stderrCap {
                    errorTail = Data(errorTail.suffix(LiveProcess.stderrCap))
                }
            }
        }
    }

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

    /// The value if it is set, without waiting.
    func peek() -> Value? {
        lock.withLock { value }
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

/// Reads a descriptor with read(2): whatever is available, at least one byte; nil at the end or on an error.
final class ChunkReader {
    private let descriptor: Int32
    private var buffer = [UInt8](repeating: 0, count: 65536)

    init(_ descriptor: Int32) {
        self.descriptor = descriptor
    }

    /// Blocks until the descriptor is readable (data, end of file or an error).
    func waitReadable() {
        var entry = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        while poll(&entry, 1, -1) < 0 && errno == EINTR {}
    }

    /// Whether the descriptor is readable now, without waiting.
    static func isReadable(_ descriptor: Int32) -> Bool {
        var entry = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        let result = poll(&entry, 1, 0)
        return result > 0 && entry.revents != 0
    }

    func next() -> Data? {
        while true {
            let count = buffer.withUnsafeMutableBytes { read(descriptor, $0.baseAddress, $0.count) }
            if count > 0 { return Data(buffer[0..<count]) }
            if count == 0 { return nil }
            if errno == EINTR { continue }
            return nil
        }
    }
}

final class LiveProcess: RunningProcess, @unchecked Sendable {
    /// Standard output kept for `stdoutAll`; a stream read through `lines` is long and not kept past this.
    private static let keepCap = 1_048_576
    private static let stderrCap = 4096
    /// How long after the exit the reader of standard error may take to reach its end before the exit is published
    /// anyway: only a helper the child left behind, holding the pipe open, makes it run out.
    private static let stderrGrace = 2.0

    let lines: AsyncThrowingStream<Data, Error>

    private let process = Process()
    private let lock = NSLock()
    private var kept = Data()
    private var errorTail = Data()
    private var finished = false
    private let outputDone = Latch<Void>()
    private let exit = Latch<Int32>()
    private var errorDrained = false
    private var exitPublished = false
    /// True while the reader of standard error is blocked in `read` with nothing taken yet; guarded by `lock`.
    private var errorReaderWaiting = false
    private var errorDescriptor: Int32 = -1

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
        process.terminationHandler = { [weak self] _ in self?.finish() }
        try process.run()

        let outHandle = outPipe.fileHandleForReading
        let errHandle = errPipe.fileHandleForReading
        errorDescriptor = errHandle.fileDescriptor
        Thread.detachNewThread { [self] in
            readOutput(outHandle, continuation)
            outputDone.set(())
        }
        Thread.detachNewThread { [self] in
            readError(errHandle)
            lock.withLock { errorDrained = true }
            publishExitIfReady()
        }
        // The termination handler is the first way to learn of the exit; this poll is the second, so a handler that
        // does not run cannot leave `exitCode` waiting for good.
        Thread.detachNewThread { [self] in
            while process.isRunning { usleep(20_000) }
            finish()
        }
    }

    /// Records the exit once, from whichever of the termination handler and the poll sees it first.
    private func finish() {
        let first: Bool = lock.withLock {
            guard !finished else { return false }
            finished = true
            return true
        }
        guard first else { return }
        publishExitIfReady()
        // The exit is published once standard error is drained: at its end, or when the reader is waiting with nothing
        // left in the pipe. A grandchild that keeps the pipe open therefore delays nothing, and a child's last words
        // are in `stderrTail` by then. A hard cap publishes it anyway.
        pollExit(deadline: Date().addingTimeInterval(LiveProcess.stderrGrace))
    }

    private func pollExit(deadline: Date) {
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.01) { [self] in
            // Drained: at its end, or the reader is waiting for the pipe to become readable and nothing is readable.
            // The reader clears `errorReaderWaiting` before it takes bytes and sets it again only after it has kept
            // them, so there is no moment with bytes taken, not kept and the flag set.
            let drained: Bool = lock.withLock {
                errorDrained || (errorReaderWaiting && !ChunkReader.isReadable(errorDescriptor))
            }
            if drained || Date() >= deadline {
                publishExit()
            } else {
                pollExit(deadline: deadline)
            }
        }
    }

    private func publishExitIfReady() {
        let ready: Bool = lock.withLock { finished && errorDrained }
        if ready { publishExit() }
    }

    private func publishExit() {
        let first: Bool = lock.withLock {
            guard !exitPublished else { return false }
            exitPublished = true
            return true
        }
        guard first else { return }
        let status = process.terminationStatus
        exit.set(process.terminationReason == .uncaughtSignal ? 128 + status : status)
    }

    private func readOutput(_ handle: FileHandle, _ continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        var splitter = LineSplitter()
        let reader = ChunkReader(handle.fileDescriptor)
        while true {
            // read(2) returns what is there; FileHandle.read(upToCount:) may wait for the whole count on a pipe and
            // hold a line back until 64 KiB have arrived.
            guard let chunk = reader.next(), !chunk.isEmpty else { break }
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
        let reader = ChunkReader(handle.fileDescriptor)
        while let chunk = reader.next(), !chunk.isEmpty {}
    }

    private func readError(_ handle: FileHandle) {
        let reader = ChunkReader(handle.fileDescriptor)
        while true {
            lock.withLock { errorReaderWaiting = true }
            let taken = reader.next()
            guard let chunk = taken, !chunk.isEmpty else { break }
            lock.withLock {
                errorReaderWaiting = false
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

    /// Returns at once: the signals are sent from a dispatch queue, so no caller waits on Foundation's process lock.
    func terminate(grace: TimeInterval) {
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            guard process.isRunning else { return }
            process.terminate()
            let pid = process.processIdentifier
            let child = process
            DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + max(grace, 0)) {
                if child.isRunning { kill(pid, SIGKILL) }
            }
        }
    }

    func exitCode() async -> Int32 {
        await exit.wait()
    }
}
