import CryptoKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

/// A fresh temporary folder, removed when the test ends.
func makeTempDirectory(_ test: XCTestCase) throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("capstan-phone-core-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    test.addTeardownBlock { try? FileManager.default.removeItem(at: url) }
    return url
}

func makePin(_ algorithm: String = "ssh-ed25519", _ byte: UInt8 = 1) -> HostKeyPin {
    HostKeyPin(algorithm: algorithm, blob: Data(repeating: byte, count: 51))
}

final class TrustTests: XCTestCase {
    private let host = UUID()

    func testNoPinIsUnknown() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .unknown(makePin()))
    }

    func testTrustedAfterTrust() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        XCTAssertTrue(store.trust(hostId: host, pin: makePin()))
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .trusted)
    }

    func testAnotherBlobIsChanged() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        store.trust(hostId: host, pin: makePin())
        let offered = makePin("ssh-ed25519", 2)
        XCTAssertEqual(store.verdict(hostId: host, offered: offered), .changed(pinned: makePin(), offered: offered))
    }

    func testAnotherAlgorithmIsChanged() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        store.trust(hostId: host, pin: makePin())
        let offered = makePin("ecdsa-sha2-nistp256", 1)
        XCTAssertEqual(store.verdict(hostId: host, offered: offered), .changed(pinned: makePin(), offered: offered))
    }

    func testTrustSetsAPinOnlyWhenThereIsNone() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        XCTAssertTrue(store.trust(hostId: host, pin: makePin()))
        XCTAssertFalse(store.trust(hostId: host, pin: makePin("ssh-ed25519", 2)), "a changed key is never re-pinned")
        XCTAssertEqual(store.pin(for: host), makePin())
    }

    func testForgetClears() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        store.trust(hostId: host, pin: makePin())
        store.forget(hostId: host)
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .unknown(makePin()))
    }

    func testSweepKeepsOnlySavedHosts() throws {
        let store = HostKeyStore(directory: try makeTempDirectory(self))
        let other = UUID()
        store.trust(hostId: host, pin: makePin())
        store.trust(hostId: other, pin: makePin())
        store.sweep(keeping: [host])
        XCTAssertEqual(store.pin(for: host), makePin())
        XCTAssertNil(store.pin(for: other))
    }

    func testReloadKeepsPins() throws {
        let directory = try makeTempDirectory(self)
        let store = HostKeyStore(directory: directory)
        let other = UUID()
        store.trust(hostId: host, pin: makePin())
        store.trust(hostId: other, pin: makePin("ecdsa-sha2-nistp256", 3))
        store.forget(hostId: other)
        store.waitForWrites()

        let reloaded = HostKeyStore(directory: directory)
        XCTAssertFalse(reloaded.isUnreadable)
        XCTAssertEqual(reloaded.verdict(hostId: host, offered: makePin()), .trusted)
        XCTAssertNil(reloaded.pin(for: other), "the last write wins")
    }

    func testCorruptFileIsUnreadableUntilResetAll() throws {
        let directory = try makeTempDirectory(self)
        let file = directory.appendingPathComponent("host-keys.json")
        let garbage = Data("{not json".utf8)
        try garbage.write(to: file)

        let store = HostKeyStore(directory: directory)
        XCTAssertTrue(store.isUnreadable)
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .pinsUnreadable)
        XCTAssertFalse(store.trust(hostId: host, pin: makePin()))
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .pinsUnreadable, "never unknown or trusted")
        store.forget(hostId: host)
        store.sweep(keeping: [])
        store.waitForWrites()
        XCTAssertEqual(try Data(contentsOf: file), garbage, "the unreadable file is left as it is")

        store.resetAll()
        XCTAssertEqual(store.verdict(hostId: host, offered: makePin()), .unknown(makePin()))
        XCTAssertTrue(store.trust(hostId: host, pin: makePin()))
        store.waitForWrites()
        XCTAssertEqual(HostKeyStore(directory: directory).verdict(hostId: host, offered: makePin()), .trusted)
    }

    func testFingerprintMatchesThePhoneKeyFormat() {
        let key = Curve25519.Signing.PrivateKey().publicKey
        let publicKey = PhonePublicKey.ed25519(key)
        let pin = HostKeyPin(algorithm: OpenSSHKey.algorithm(publicKey), blob: OpenSSHKey.blob(publicKey))
        XCTAssertEqual(pin.fingerprint, OpenSSHKey.fingerprint(publicKey))
    }
}
