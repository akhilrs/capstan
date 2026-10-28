import XCTest
import CapstanKit

/// Uses the public interface only, as the app does.
final class HostConfigTests: XCTestCase {
    func testDefaults() {
        let config = HostConfig(displayName: "Box", destination: "box")
        XCTAssertEqual(config.cstanPath, "cstan")
        XCTAssertEqual(config.extraRoots, [])
        XCTAssertEqual(config.pollMs, 2000)
        XCTAssertNil(config.identityFile)
        XCTAssertTrue(config.enabled)
        XCTAssertTrue(config.validate().isEmpty)
        XCTAssertNotEqual(config.id, HostConfig(displayName: "Box", destination: "box").id)
    }

    func testCodableRoundTrip() throws {
        let config = HostConfig(
            displayName: "Box", destination: "user@box", cstanPath: "~/bin/cstan", extraRoots: ["/work"],
            pollMs: 3000, identityFile: "/keys/id", enabled: false)
        let data = try JSONEncoder().encode(config)
        XCTAssertEqual(try JSONDecoder().decode(HostConfig.self, from: data), config)
    }

    func testPublicSurface() {
        let caps = HostCapabilities(reachable: true, events: true, decide: false)
        XCTAssertNil(caps.error)
        let check = HostCheck(step: .connect, status: .passed, text: "ok")
        XCTAssertEqual(check.step, .connect)
        _ = LiveProcessRunner()
        _ = RemoteCommand.ssh(HostConfig(displayName: "Box", destination: "box"), ["events"])
    }
}
