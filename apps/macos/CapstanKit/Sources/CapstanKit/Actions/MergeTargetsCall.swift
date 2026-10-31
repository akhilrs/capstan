import Foundation

/// The branches a merge of one project may go into, as `cstan merge-targets` lists them. Branch names are host text,
/// cleaned; they are shown as plain text and sent back only as the value of `--into`.
public struct MergeTargets: Sendable, Equatable {
    /// `[remote] merge` is on.
    public let enabled: Bool
    /// Where a merge without `--into` goes; nil when merging is off or no default is known.
    public let defaultBranch: String?
    /// The branch checked out in the project's root; nil on a detached HEAD.
    public let checkedOut: String?
    /// The branches `--into` may name, sorted.
    public let branches: [String]

    public init(enabled: Bool, defaultBranch: String?, checkedOut: String?, branches: [String]) {
        self.enabled = enabled
        self.defaultBranch = defaultBranch
        self.checkedOut = checkedOut
        self.branches = branches
    }

    /// The branch to offer first: the default when it can be chosen, else the checked-out one, else the first.
    public var preferred: String? {
        if let defaultBranch, branches.contains(defaultBranch) { return defaultBranch }
        if let checkedOut, branches.contains(checkedOut) { return checkedOut }
        return branches.first
    }
}

/// What a call to `cstan merge-targets` came to.
public enum MergeTargetsResult: Sendable, Equatable {
    case targets(MergeTargets)
    /// The host's cstan has no `merge-targets`: the merge goes into the project's branch only.
    case unsupported
    /// A refusal or no answer; the text is host text, cleaned.
    case error(String)
}

/// `cstan merge-targets --project <id> --json` over ssh, with its answer parsed.
public struct MergeTargetsCall: Sendable, Equatable {
    public static let timeoutSeconds: TimeInterval = 25
    static let unknownCommandText = "cstan: unknown command: merge-targets"

    public let projectId: String

    public init(projectId: String) {
        self.projectId = projectId
    }

    public func remoteArguments() throws -> [String] {
        guard RecentCall.isProjectId(projectId) else { throw RecentCallError.project }
        return ["merge-targets", "--project", projectId, "--json"]
    }

    public func sshArguments(_ config: HostConfig) throws -> [String] {
        RemoteCommand.ssh(config, try remoteArguments())
    }

    private enum Finish {
        case done(stdout: Data, code: Int32)
        case timeout
    }

    /// Starts the call and waits at most `timeoutSeconds`; never throws.
    public func run(runner: any ProcessRunner, config: HostConfig, clock: any SessionClock) async -> MergeTargetsResult {
        let arguments: [String]
        do {
            arguments = try sshArguments(config)
        } catch {
            return .error("The branch list request is not valid")
        }
        let process: any RunningProcess
        do {
            process = try runner.start(URL(fileURLWithPath: RemoteCommand.sshPath), arguments)
        } catch {
            return .error("The branch list could not be read")
        }
        let timedOut = Latch<Bool>()
        let finish: Finish = await withTaskGroup(of: Finish?.self) { group in
            group.addTask {
                let stdout = await process.stdoutAll()
                return .done(stdout: stdout, code: await process.exitCode())
            }
            group.addTask {
                do { try await clock.sleep(seconds: MergeTargetsCall.timeoutSeconds) } catch { return nil }
                timedOut.set(true)
                process.terminate()
                return .timeout
            }
            var result: Finish = .timeout
            for await item in group {
                if let item {
                    result = item
                    group.cancelAll()
                    break
                }
            }
            return result
        }
        if timedOut.peek() != nil { return .error("The host did not answer in time") }
        switch finish {
        case .timeout:
            return .error("The host did not answer in time")
        case .done(let stdout, let code):
            return MergeTargetsCall.result(stdout: stdout, stderr: process.stderrTail(), exitCode: code)
        }
    }

    /// Reads what the child printed: exit 0 is the list, exit 1 a refusal as one JSON line, a cstan without the
    /// command is `unsupported`.
    public static func result(stdout: Data, stderr: String, exitCode: Int32) -> MergeTargetsResult {
        if exitCode == 255 { return .error(plain(stderr)) }
        let printed = String(decoding: stdout, as: UTF8.self)
        if printed.contains(unknownCommandText) || stderr.contains(unknownCommandText) { return .unsupported }
        guard let value = lastObject(stdout) else {
            return .error(plain(stderr.isEmpty ? printed : stderr))
        }
        if case .object? = value["error"] {
            let code = text(value["error"]?["code"]) ?? ""
            if code == "unknown_command" { return .unsupported }
            return .error(plain(text(value["error"]?["message"]) ?? code))
        }
        guard case .array(let rows)? = value["branches"] else { return .error("The branch list is not valid") }
        var enabled = false
        if case .bool(let flag)? = value["enabled"] { enabled = flag }
        let branches = rows.compactMap { text($0) }.map(plain).filter { !$0.isEmpty && !$0.hasPrefix("-") }
        return .targets(MergeTargets(
            enabled: enabled, defaultBranch: text(value["defaultBranch"]).map(plain),
            checkedOut: text(value["checkedOut"]).map(plain), branches: branches))
    }

    private static func text(_ value: JSONValue?) -> String? {
        if case .string(let text)? = value { return text }
        return nil
    }

    private static func lastObject(_ stdout: Data) -> JSONValue? {
        let lines = stdout.split(separator: 0x0A, omittingEmptySubsequences: true)
        guard let line = lines.last,
              let value = try? JSONDecoder().decode(JSONValue.self, from: Data(line)),
              case .object = value else { return nil }
        return value
    }

    private static func plain(_ text: String) -> String {
        String(UntrustedText(raw: text, verified: true, truncated: false).displayPlain.prefix(255))
    }
}
