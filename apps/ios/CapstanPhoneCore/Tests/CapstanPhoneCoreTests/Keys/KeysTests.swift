import CryptoKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

/// Keys in memory, with a switch that makes `remove(id:)` fail.
final class MemoryKeyStorage: KeyStorage, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [StoredKey] = []
    private var failRemove = false

    func setFailRemove(_ value: Bool) { lock.withLock { failRemove = value } }

    func all() throws -> [StoredKey] { lock.withLock { keys } }

    func add(_ key: StoredKey) throws { lock.withLock { keys.append(key) } }

    func remove(id: String) throws {
        try lock.withLock {
            if failRemove { throw KeyVaultError.keychain(-1) }
            keys.removeAll { $0.id == id }
        }
    }

    func removeAll() throws { lock.withLock { keys = [] } }
}

/// Real ed25519 keys; no Secure Enclave, as under `swift test` (enclave create and signing are device checks).
struct NoEnclaveKeyMaker: KeyMaker {
    func make(_ kind: PhoneKeyKind) throws -> (key: PhoneKey, data: Data) {
        switch kind {
        case .secureEnclaveP256:
            throw KeyVaultError.enclaveUnavailable
        case .keychainEd25519:
            let key = Curve25519.Signing.PrivateKey()
            return (PhoneKey(.keychainEd25519(key)), key.rawRepresentation)
        }
    }

    func load(_ kind: PhoneKeyKind, data: Data) throws -> PhoneKey {
        switch kind {
        case .secureEnclaveP256:
            throw KeyVaultError.enclaveUnavailable
        case .keychainEd25519:
            guard let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: data) else {
                throw KeyVaultError.unreadable
            }
            return PhoneKey(.keychainEd25519(key))
        }
    }
}

final class KeysTests: XCTestCase {
    private let fixedNow = Date(timeIntervalSince1970: 1_800_000_000)

    private func makeVault(_ storage: MemoryKeyStorage) -> KeychainKeyVault {
        let now = fixedNow
        return KeychainKeyVault(storage: storage, maker: NoEnclaveKeyMaker(), now: { now })
    }

    // MARK: Vault

    func testCreateCurrentDeleteEd25519() throws {
        let storage = MemoryKeyStorage()
        let vault = makeVault(storage)
        XCTAssertNil(try vault.current())

        let key = try vault.create(.keychainEd25519)
        XCTAssertEqual(key.kind, .keychainEd25519)
        XCTAssertEqual(try vault.current()?.publicKey, key.publicKey)
        XCTAssertThrowsError(try vault.create(.keychainEd25519)) { XCTAssertEqual($0 as? KeyVaultError, .keyExists) }

        try vault.delete()
        XCTAssertNil(try vault.current())
        XCTAssertNoThrow(try vault.delete(), "deleting no key is not an error")
    }

    func testEnclaveUnavailableIsAnErrorNotAnEd25519Key() throws {
        let storage = MemoryKeyStorage()
        let vault = makeVault(storage)
        XCTAssertThrowsError(try vault.create(.secureEnclaveP256)) {
            XCTAssertEqual($0 as? KeyVaultError, .enclaveUnavailable)
        }
        XCTAssertTrue(try storage.all().isEmpty)
        XCTAssertNil(try vault.current())
    }

    func testReplaceMakesANewKeyAndRemovesTheOld() throws {
        let storage = MemoryKeyStorage()
        let vault = makeVault(storage)
        let old = try vault.create(.keychainEd25519)
        let new = try vault.replace(.keychainEd25519)
        XCTAssertNotEqual(new.publicKey, old.publicKey)
        XCTAssertEqual(try vault.current()?.publicKey, new.publicKey)
        XCTAssertEqual(try storage.all().count, 1)
    }

    func testReplaceThatCannotCreateKeepsTheOldKey() throws {
        let storage = MemoryKeyStorage()
        let vault = makeVault(storage)
        let old = try vault.create(.keychainEd25519)
        XCTAssertThrowsError(try vault.replace(.secureEnclaveP256)) {
            XCTAssertEqual($0 as? KeyVaultError, .enclaveUnavailable)
        }
        XCTAssertEqual(try vault.current()?.publicKey, old.publicKey)
        XCTAssertEqual(try storage.all().count, 1)
    }

    func testReplaceWhoseCleanupFailsStillUsesTheNewKey() throws {
        let storage = MemoryKeyStorage()
        let vault = makeVault(storage)
        _ = try vault.create(.keychainEd25519)
        storage.setFailRemove(true)
        let new = try vault.replace(.keychainEd25519)
        XCTAssertEqual(try storage.all().count, 2)
        XCTAssertEqual(try vault.current()?.publicKey, new.publicKey, "the newest stored key is the key")
    }

    func testUnreadableStoredKey() throws {
        let storage = MemoryKeyStorage()
        try storage.add(StoredKey(id: "x", kind: .keychainEd25519, created: fixedNow, data: Data([1, 2, 3])))
        XCTAssertThrowsError(try makeVault(storage).current()) { XCTAssertEqual($0 as? KeyVaultError, .unreadable) }
    }

    // MARK: ssh-keygen output

    private func fixture(_ name: String) throws -> String {
        let folder = try XCTUnwrap(Bundle.module.url(forResource: "Fixtures", withExtension: nil))
        let text = try String(contentsOf: folder.appendingPathComponent(name), encoding: .utf8)
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func testP256MatchesSSHKeygen() throws {
        // A software P-256 key: its public key type is the Secure Enclave key's.
        let key = try P256.Signing.PrivateKey(pemRepresentation: try fixture("p256.test-key"))
        let publicKey = PhonePublicKey.p256(key.publicKey)
        // ssh-keygen -y prints no comment for a PEM key.
        XCTAssertEqual(OpenSSHKey.publicLine(publicKey), try fixture("p256.public.txt"))
        XCTAssertEqual(OpenSSHKey.fingerprint(publicKey), try fixture("p256.fingerprint.txt"))
        XCTAssertEqual(OpenSSHKey.algorithm(publicKey), "ecdsa-sha2-nistp256")
    }

    func testEd25519MatchesSSHKeygen() throws {
        let seed = try XCTUnwrap(Data(hex: try fixture("ed25519-seed.test-key")))
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
        let publicKey = PhonePublicKey.ed25519(key.publicKey)
        XCTAssertEqual(OpenSSHKey.publicLine(publicKey, comment: "capstan-test"), try fixture("ed25519.public.txt"))
        XCTAssertEqual(OpenSSHKey.fingerprint(publicKey), try fixture("ed25519.fingerprint.txt"))
        XCTAssertEqual(OpenSSHKey.algorithm(publicKey), "ssh-ed25519")
    }

    func testPhoneKeyReportsItsPublicKey() throws {
        let seed = try XCTUnwrap(Data(hex: try fixture("ed25519-seed.test-key")))
        let key = PhoneKey(.keychainEd25519(try Curve25519.Signing.PrivateKey(rawRepresentation: seed)))
        XCTAssertEqual(key.kind, .keychainEd25519)
        XCTAssertEqual(OpenSSHKey.publicLine(key.publicKey, comment: "capstan-test"), try fixture("ed25519.public.txt"))
    }
}

extension Data {
    /// Bytes from lowercase or uppercase hex; nil for an odd length or a non-hex character.
    init?(hex: String) {
        let digits = Array(hex.utf8)
        guard digits.count % 2 == 0 else { return nil }
        var bytes: [UInt8] = []
        bytes.reserveCapacity(digits.count / 2)
        var index = 0
        while index < digits.count {
            guard let high = Data.nibble(digits[index]), let low = Data.nibble(digits[index + 1]) else { return nil }
            bytes.append(high << 4 | low)
            index += 2
        }
        self.init(bytes)
    }

    private static func nibble(_ digit: UInt8) -> UInt8? {
        switch digit {
        case 0x30...0x39: return digit - 0x30
        case 0x61...0x66: return digit - 0x61 + 10
        case 0x41...0x46: return digit - 0x41 + 10
        default: return nil
        }
    }
}
