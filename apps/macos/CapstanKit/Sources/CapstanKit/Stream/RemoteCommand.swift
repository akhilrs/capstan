import Foundation

public enum ShellQuote {
    /// POSIX single quotes; a quote inside the text is written as `'\''`.
    public static func quote(_ text: String) -> String {
        "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

/// One ssh call read back from its argument list: what an SSH client that is not `/usr/bin/ssh` needs to run it.
public struct SSHInvocation: Sendable, Equatable {
    public var destination: String
    public var identityFile: String?
    public var remote: String

    public init(destination: String, identityFile: String?, remote: String) {
        self.destination = destination
        self.identityFile = identityFile
        self.remote = remote
    }
}

/// The ssh command lines of the client. The user's own ssh, config and agent run them; BatchMode means no prompt.
public enum RemoteCommand {
    public static let sshPath = "/usr/bin/ssh"

    /// The arguments of `cstan events` for one connection.
    public static func eventsArguments(_ config: HostConfig, cursor: Cursor?) -> [String] {
        var arguments = ["events", "--poll-ms", String(config.pollMs)]
        if let cursor { arguments += ["--cursor", cursor.text] }
        for root in config.extraRoots { arguments += ["--root", root] }
        return arguments
    }

    /// The argument list for `/usr/bin/ssh` that runs `cstan <remoteArgs>` on the host.
    public static func ssh(_ config: HostConfig, _ remoteArgs: [String]) -> [String] {
        sshArguments(config, remote: remoteCommand(config, remoteArgs))
    }

    /// The argument list for `/usr/bin/ssh` that runs the given command line on the host.
    public static func sshArguments(_ config: HostConfig, remote: String) -> [String] {
        var arguments = fixedOptions
        if let identityFile = config.identityFile {
            arguments += identityOptions + [identityFile]
        }
        arguments += ["--", config.destination, remote]
        return arguments
    }

    /// The options `sshArguments` writes before the optional key file, in order.
    private static let fixedOptions = [
        "-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
    ]
    private static let identityOptions = ["-o", "IdentitiesOnly=yes", "-i"]

    /// The exact inverse of `sshArguments`: nil for any list it does not write (another option, another order, no
    /// `--`, or more than one remote word).
    public static func invocation(from arguments: [String]) -> SSHInvocation? {
        guard arguments.starts(with: fixedOptions) else { return nil }
        var rest = arguments.dropFirst(fixedOptions.count)
        var identityFile: String?
        if rest.starts(with: identityOptions) {
            rest = rest.dropFirst(identityOptions.count)
            guard let file = rest.first else { return nil }
            identityFile = file
            rest = rest.dropFirst()
        }
        let tail = Array(rest)
        guard tail.count == 3, tail[0] == "--" else { return nil }
        return SSHInvocation(destination: tail[1], identityFile: identityFile, remote: tail[2])
    }

    /// The remote command line: the cstan path (a leading `~/` stays unquoted) and each argument quoted.
    public static func remoteCommand(_ config: HostConfig, _ remoteArgs: [String]) -> String {
        ([cstanWord(config)] + remoteArgs.map(ShellQuote.quote)).joined(separator: " ")
    }

    /// The cstan path as one shell word.
    public static func cstanWord(_ config: HostConfig) -> String {
        if config.cstanPath.hasPrefix("~/") {
            return "~/" + ShellQuote.quote(String(config.cstanPath.dropFirst(2)))
        }
        return ShellQuote.quote(config.cstanPath)
    }

    /// Wraps a script as `sh -c '<script>'` so it runs under sh whatever the login shell is.
    public static func shScript(_ script: String) -> String {
        "sh -c " + ShellQuote.quote(script)
    }
}
