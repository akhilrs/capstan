import XCTest
@testable import CapstanKit

/// `RemoteCommand.invocation(from:)` reads back exactly what `sshArguments` writes, and nothing else.
final class InvocationTests: XCTestCase {
    private let remotes = [
        "true",
        "'cstan' 'events' '--poll-ms' '2000'",
        "~/'bin/cstan' 'events' '--root' '/work/it'\\''s here'",
        RemoteCommand.shScript("cstan help events >/dev/null 2>&1 && echo cap events; true"),
        RemoteCommand.shScript("echo 'first line'\nprintf '%s\\n' \"it's\"\ntrue"),
        "",
    ]

    func testRoundTripWithoutKeyFile() {
        let config = makeConfig()
        for remote in remotes {
            let arguments = RemoteCommand.sshArguments(config, remote: remote)
            XCTAssertEqual(
                RemoteCommand.invocation(from: arguments),
                SSHInvocation(destination: "user@box", identityFile: nil, remote: remote), remote)
        }
    }

    func testRoundTripWithKeyFile() {
        let config = makeConfig(destination: "build-box.example", identityFile: "/keys/it's id")
        for remote in remotes {
            let arguments = RemoteCommand.sshArguments(config, remote: remote)
            XCTAssertEqual(
                RemoteCommand.invocation(from: arguments),
                SSHInvocation(destination: "build-box.example", identityFile: "/keys/it's id", remote: remote), remote)
        }
    }

    func testRejectsListsSshArgumentsDoesNotWrite() {
        let good = RemoteCommand.sshArguments(makeConfig(identityFile: "/keys/id"), remote: "true")
        XCTAssertNotNil(RemoteCommand.invocation(from: good))

        var extra = good
        extra.insert(contentsOf: ["-o", "ProxyCommand=x"], at: 7)
        XCTAssertNil(RemoteCommand.invocation(from: extra), "extra option")

        var missing = good
        missing.removeSubrange(1...2)
        XCTAssertNil(RemoteCommand.invocation(from: missing), "missing option")

        var reordered = good
        reordered.swapAt(2, 4)
        XCTAssertNil(RemoteCommand.invocation(from: reordered), "reordered options")

        let keyFirst = Array(good[7...10]) + Array(good[0...6]) + Array(good[11...])
        XCTAssertNil(RemoteCommand.invocation(from: keyFirst), "key file before the fixed options")

        var noKeyFile = Array(good[0...9])
        noKeyFile += ["--", "user@box", "true"]
        XCTAssertNil(RemoteCommand.invocation(from: noKeyFile), "-i without a key file")

        var noSeparator = good
        noSeparator.removeAll { $0 == "--" }
        XCTAssertNil(RemoteCommand.invocation(from: noSeparator), "missing --")

        XCTAssertNil(RemoteCommand.invocation(from: good + ["second"]), "second remote word")
        XCTAssertNil(RemoteCommand.invocation(from: Array(good.dropLast())), "no remote")
        XCTAssertNil(RemoteCommand.invocation(from: []), "empty list")
    }

    /// Every argument list a caller in the Kit hands to `runner.start` reads back.
    func testEveryCallerArgvParses() throws {
        for config in [makeConfig(), makeConfig(roots: ["/work/a"], identityFile: "/keys/id", cstanPath: "~/bin/cstan")] {
            let cursor = makeCursor(["p1"])
            // HostSession and the check's events run.
            for arguments in [
                RemoteCommand.ssh(config, RemoteCommand.eventsArguments(config, cursor: nil)),
                RemoteCommand.ssh(config, RemoteCommand.eventsArguments(config, cursor: cursor)),
            ] {
                let invocation = try XCTUnwrap(RemoteCommand.invocation(from: arguments))
                XCTAssertEqual(invocation.destination, config.destination)
                XCTAssertEqual(invocation.identityFile, config.identityFile)
            }
            // Probe: the capability script and the reach check.
            let cstan = RemoteCommand.cstanWord(config)
            let script = "\(cstan) help events >/dev/null 2>&1 && echo cap events; "
                + "\(cstan) help decide >/dev/null 2>&1 && echo cap decide; true"
            let probe = RemoteCommand.sshArguments(config, remote: RemoteCommand.shScript(script))
            XCTAssertEqual(RemoteCommand.invocation(from: probe)?.remote, RemoteCommand.shScript(script))
            XCTAssertEqual(RemoteCommand.invocation(from: RemoteCommand.sshArguments(config, remote: "true"))?.remote, "true")
            // Decider.
            for verb: DecideVerb in [.opDeny(note: "it's not safe"), .promptOption(2), .messageReply("line one\nline two")] {
                let arguments = try makeRequest(verb: verb).sshArguments(config)
                let invocation = try XCTUnwrap(RemoteCommand.invocation(from: arguments))
                XCTAssertEqual(invocation.remote, arguments.last)
            }
            // RecentCall.
            let recent = try RecentCall(projectId: "proj-1", epoch: "0123456789abcdef", head: 7).sshArguments(config)
            XCTAssertEqual(RemoteCommand.invocation(from: recent)?.remote, recent.last)
        }
    }

    /// The callers above are all the places that start ssh; a new one must be added to the test above.
    func testNoOtherStartCallSites() throws {
        let root = RepoPaths.repoRoot.appendingPathComponent("apps/macos")
        var found: Set<String> = []
        for package in ["CapstanKit/Sources", "CapstanAppCore/Sources"] {
            let base = root.appendingPathComponent(package)
            let walker = try XCTUnwrap(FileManager.default.enumerator(at: base, includingPropertiesForKeys: nil))
            for case let url as URL in walker where url.pathExtension == "swift" {
                let text = try String(contentsOf: url, encoding: .utf8)
                if text.contains("runner.start(") { found.insert(url.deletingPathExtension().lastPathComponent) }
            }
        }
        XCTAssertEqual(found, ["Decider", "HostSession", "Probe", "RecentCall"])
    }
}
