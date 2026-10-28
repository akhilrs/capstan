import XCTest
@testable import CapstanKit

final class RemoteCommandTests: XCTestCase {
    func testQuote() {
        XCTAssertEqual(ShellQuote.quote("plain"), "'plain'")
        XCTAssertEqual(ShellQuote.quote("it's"), "'it'\\''s'")
        XCTAssertEqual(ShellQuote.quote(""), "''")
    }

    func testExactArgvWithIdentityRootsAndCursor() throws {
        let config = makeConfig(roots: ["/work/a", "/work/it's here"], identityFile: "/keys/id")
        let cursor = try XCTUnwrap(Cursor(text: "c1.abc"))
        let args = RemoteCommand.eventsArguments(config, cursor: cursor)
        let remote = "'cstan' 'events' '--poll-ms' '2000' '--cursor' 'c1.abc' '--root' '/work/a' "
            + "'--root' '/work/it'\\''s here'"
        XCTAssertEqual(RemoteCommand.ssh(config, args), [
            "-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
            "-o", "IdentitiesOnly=yes", "-i", "/keys/id", "--", "user@box", remote,
        ])
        XCTAssertEqual(try ShParser.split(remote), [
            "cstan", "events", "--poll-ms", "2000", "--cursor", "c1.abc", "--root", "/work/a",
            "--root", "/work/it's here",
        ])
    }

    func testNoIdentityNoCursor() {
        let config = makeConfig(pollMs: 5000)
        XCTAssertEqual(RemoteCommand.ssh(config, RemoteCommand.eventsArguments(config, cursor: nil)), [
            "-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
            "--", "user@box", "'cstan' 'events' '--poll-ms' '5000'",
        ])
    }

    func testHomeRelativePathKeepsTildeUnquoted() throws {
        let config = makeConfig(cstanPath: "~/bin/cstan")
        let remote = RemoteCommand.remoteCommand(config, ["events"])
        XCTAssertTrue(remote.hasPrefix("~/'bin/cstan' "), remote)
        XCTAssertEqual(try ShParser.split(remote), ["~/bin/cstan", "events"])
    }

    func testShScriptRunsUnderSh() throws {
        let script = "cstan help events >/dev/null 2>&1 && echo cap events; echo 'it'\"s\""
        let wrapped = RemoteCommand.shScript(script)
        XCTAssertEqual(try ShParser.split(wrapped), ["sh", "-c", script])
    }

    func testValidateRejects() {
        XCTAssertTrue(makeConfig().validate().isEmpty)
        XCTAssertEqual(makeConfig(destination: "-oProxyCommand=x").validate(), [.destination])
        XCTAssertEqual(makeConfig(cstanPath: "-x").validate(), [.cstanPath])
        XCTAssertEqual(makeConfig(roots: ["~/work"]).validate(), [.root("~/work")])
        XCTAssertEqual(makeConfig(roots: ["/work/a\nb"]).validate(), [.root("/work/a\nb")])
        XCTAssertEqual(makeConfig(pollMs: 999).validate(), [.pollMs])
        XCTAssertEqual(makeConfig(identityFile: "keys/id").validate(), [.identityFile])
    }

    func testValidateAccepts() {
        XCTAssertTrue(makeConfig(destination: "user@[fe80::1%en0]").validate().isEmpty)
        XCTAssertTrue(makeConfig(destination: "build-box.example").validate().isEmpty)
        XCTAssertTrue(makeConfig(cstanPath: "~/bin/cstan").validate().isEmpty)
        XCTAssertTrue(makeConfig(cstanPath: "/usr/local/bin/cstan").validate().isEmpty)
        XCTAssertTrue(makeConfig(pollMs: 1000, identityFile: "/keys/id").validate().isEmpty)
        XCTAssertEqual(HostConfigError.pollMs.message, "At least 1000 ms")
    }

    func testValidateRejectsMore() {
        XCTAssertEqual(makeConfig(destination: "").validate(), [.destination])
        XCTAssertEqual(makeConfig(destination: "a b").validate(), [.destination])
        XCTAssertEqual(makeConfig(destination: "a;b").validate(), [.destination])
        XCTAssertEqual(makeConfig(destination: String(repeating: "a", count: 256)).validate(), [.destination])
        XCTAssertEqual(makeConfig(cstanPath: "").validate(), [.cstanPath])
        XCTAssertEqual(makeConfig(cstanPath: "cstan; rm").validate(), [.cstanPath])
        XCTAssertEqual(makeConfig(cstanPath: "~/").validate(), [.cstanPath])
    }
}
