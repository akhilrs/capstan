import Foundation

public enum ShellQuote {
    /// POSIX single quotes; a quote inside the text is written as `'\''`.
    public static func quote(_ text: String) -> String {
        "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'"
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
        var arguments = [
            "-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
        ]
        if let identityFile = config.identityFile {
            arguments += ["-o", "IdentitiesOnly=yes", "-i", identityFile]
        }
        arguments += ["--", config.destination, remote]
        return arguments
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
