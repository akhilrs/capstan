import CapstanKit
import Foundation

/// A value set once that any number of tasks can wait for.
final class OnceValue<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    /// Sets the value; later calls change nothing. True for the call that set it.
    @discardableResult
    func set(_ newValue: Value) -> Bool {
        let resumed: [CheckedContinuation<Value, Never>]? = lock.withLock {
            guard value == nil else { return nil }
            value = newValue
            let pending = waiters
            waiters = []
            return pending
        }
        guard let resumed else { return false }
        for waiter in resumed { waiter.resume(returning: newValue) }
        return true
    }

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

/// One remote command on an SSH exec channel, behaving as the Mac's `LiveProcess`: lines through `LineSplitter`,
/// at most 1 MiB kept for `stdoutAll`, the last 4096 bytes of standard error, and 255 with a one-line tail when the
/// connection, auth, host key or channel failed.
final class SSHProcess: RunningProcess, @unchecked Sendable {
    static let keepCap = 1_048_576
    static let stderrCap = 4096
    static let failureCode: Int32 = 255

    let lines: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    private let lock = NSLock()
    private var splitter = LineSplitter()
    private var kept = Data()
    private var errorTail = Data()
    /// Set after a line over the cap: the rest of standard output is dropped.
    private var linesFailed = false
    private var exitStatus: Int32?
    private var terminator: (@Sendable (TimeInterval) -> Void)?
    private var pendingTerminate: TimeInterval?
    private let exit = OnceValue<Int32>()

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        lines = stream
        self.continuation = continuation
    }

    // MARK: From the channel (event loop)

    func receiveStdout(_ chunk: Data) {
        let (newLines, failure): ([Data], Error?) = lock.withLock {
            guard !linesFailed else { return ([], nil) }
            if kept.count < SSHProcess.keepCap { kept.append(chunk.prefix(SSHProcess.keepCap - kept.count)) }
            do {
                return (try splitter.feed(chunk), nil)
            } catch {
                linesFailed = true
                return ([], error)
            }
        }
        for line in newLines { continuation.yield(line) }
        if let failure {
            continuation.finish(throwing: failure)
            terminate(grace: 3)
        }
    }

    func receiveStderr(_ chunk: Data) {
        lock.withLock {
            errorTail.append(chunk)
            if errorTail.count > SSHProcess.stderrCap { errorTail = Data(errorTail.suffix(SSHProcess.stderrCap)) }
        }
    }

    /// The remote's `exit-status`, or `128 + n` for its `exit-signal`.
    func receiveExit(_ code: Int32) {
        lock.withLock { if exitStatus == nil { exitStatus = code } }
    }

    /// Lets `terminate` reach the channel. A terminate asked for before this runs is applied now.
    func attach(_ terminate: @escaping @Sendable (TimeInterval) -> Void) {
        let pending: TimeInterval? = lock.withLock {
            terminator = terminate
            return pendingTerminate
        }
        if let pending { terminate(pending) }
    }

    /// The channel ended: the exit code is the remote's, else 255 with `failure` as the tail.
    func finish(failure: String) {
        let code: Int32 = lock.withLock {
            if let exitStatus { return exitStatus }
            errorTail = Data((failure + "\n").utf8)
            return SSHProcess.failureCode
        }
        continuation.finish()
        exit.set(code)
    }

    // MARK: RunningProcess

    func stdoutAll() async -> Data {
        _ = await exit.wait()
        return lock.withLock { kept }
    }

    func stderrTail() -> String {
        String(decoding: lock.withLock { errorTail }, as: UTF8.self)
    }

    func terminate(grace: TimeInterval) {
        let target: (@Sendable (TimeInterval) -> Void)? = lock.withLock {
            if terminator == nil, pendingTerminate == nil { pendingTerminate = grace }
            return terminator
        }
        target?(grace)
    }

    func exitCode() async -> Int32 {
        await exit.wait()
    }
}
