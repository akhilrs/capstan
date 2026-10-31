import CryptoKit
import Foundation
import Security

/// One stored key: its bytes and what is needed to read them back.
public struct StoredKey: Sendable, Equatable {
    public let id: String
    public let kind: PhoneKeyKind
    public let created: Date
    /// The ed25519 raw key, or the enclave key's `dataRepresentation` (usable only by this device's enclave).
    public let data: Data

    public init(id: String, kind: PhoneKeyKind, created: Date, data: Data) {
        self.id = id
        self.kind = kind
        self.created = created
        self.data = data
    }
}

/// Where stored keys are kept. The Keychain on the phone; memory in tests.
public protocol KeyStorage: Sendable {
    func all() throws -> [StoredKey]
    func add(_ key: StoredKey) throws
    func remove(id: String) throws
    func removeAll() throws
}

/// Makes keys and reads stored ones back. CryptoKit and the Secure Enclave on the phone; a fake in tests.
public protocol KeyMaker: Sendable {
    /// A new key and the bytes to store for it.
    func make(_ kind: PhoneKeyKind) throws -> (key: PhoneKey, data: Data)
    func load(_ kind: PhoneKeyKind, data: Data) throws -> PhoneKey
}

/// The phone's key vault. When several keys are stored (a `replace` whose cleanup failed) the newest is the key.
public struct KeychainKeyVault: KeyVault {
    private let storage: any KeyStorage
    private let maker: any KeyMaker
    private let now: @Sendable () -> Date

    public init(
        storage: any KeyStorage = KeychainKeyStorage(), maker: any KeyMaker = DeviceKeyMaker(),
        now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.storage = storage
        self.maker = maker
        self.now = now
    }

    public func current() throws -> PhoneKey? {
        guard let newest = try newestStored() else { return nil }
        return try maker.load(newest.kind, data: newest.data)
    }

    public func create(_ kind: PhoneKeyKind) throws -> PhoneKey {
        guard try storage.all().isEmpty else { throw KeyVaultError.keyExists }
        return try makeAndStore(kind)
    }

    public func delete() throws {
        try storage.removeAll()
    }

    public func replace(_ kind: PhoneKeyKind) throws -> PhoneKey {
        let old = try storage.all().map(\.id)
        let key = try makeAndStore(kind)
        // The new key is the newest, so it is current even if removing an old one fails here.
        for id in old { try? storage.remove(id: id) }
        return key
    }

    private func newestStored() throws -> StoredKey? {
        try storage.all().max { $0.created < $1.created }
    }

    private func makeAndStore(_ kind: PhoneKeyKind) throws -> PhoneKey {
        let made = try maker.make(kind)
        // Strictly newer than any stored key, even when the clock has not moved since the last one.
        var created = now()
        if let newest = try newestStored(), created <= newest.created {
            created = newest.created.addingTimeInterval(0.001)
        }
        try storage.add(StoredKey(id: UUID().uuidString, kind: kind, created: created, data: made.data))
        return made.key
    }
}

// MARK: CryptoKit and the Secure Enclave

/// The enclave key is created with `AfterFirstUnlockThisDeviceOnly` and `.privateKeyUsage`, no biometry flag: Face ID
/// is `FaceIDRule`'s job, not each signature's.
public struct DeviceKeyMaker: KeyMaker {
    public init() {}

    public func make(_ kind: PhoneKeyKind) throws -> (key: PhoneKey, data: Data) {
        switch kind {
        case .secureEnclaveP256:
            guard SecureEnclave.isAvailable else { throw KeyVaultError.enclaveUnavailable }
            guard let access = SecAccessControlCreateWithFlags(
                nil, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, .privateKeyUsage, nil)
            else {
                throw KeyVaultError.keychain(errSecParam)
            }
            do {
                let key = try SecureEnclave.P256.Signing.PrivateKey(compactRepresentable: false, accessControl: access)
                return (PhoneKey(.secureEnclaveP256(key)), key.dataRepresentation)
            } catch {
                throw KeyVaultError.enclaveUnavailable
            }
        case .keychainEd25519:
            let key = Curve25519.Signing.PrivateKey()
            return (PhoneKey(.keychainEd25519(key)), key.rawRepresentation)
        }
    }

    public func load(_ kind: PhoneKeyKind, data: Data) throws -> PhoneKey {
        switch kind {
        case .secureEnclaveP256:
            guard SecureEnclave.isAvailable else { throw KeyVaultError.enclaveUnavailable }
            guard let key = try? SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: data) else {
                throw KeyVaultError.unreadable
            }
            return PhoneKey(.secureEnclaveP256(key))
        case .keychainEd25519:
            guard let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: data) else {
                throw KeyVaultError.unreadable
            }
            return PhoneKey(.keychainEd25519(key))
        }
    }
}

// MARK: Keychain

/// Each key is one generic-password item under `service`. The enclave key's bytes are stored with
/// `AfterFirstUnlockThisDeviceOnly`, the ed25519 key with `AfterFirstUnlock`; neither is synchronizable.
public struct KeychainKeyStorage: KeyStorage {
    public static let defaultService = "capstan.phone-key"

    private let service: String

    public init(service: String = KeychainKeyStorage.defaultService) {
        self.service = service
    }

    /// What is kept next to the key bytes, in the item's generic attribute.
    private struct Info: Codable {
        let kind: PhoneKeyKind
        let created: Date
    }

    private func baseQuery() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    public func all() throws -> [StoredKey] {
        var query = baseQuery()
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        query[kSecReturnAttributes as String] = true
        query[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else { throw KeyVaultError.keychain(status) }
        guard let items = result as? [[String: Any]] else { throw KeyVaultError.unreadable }
        return try items.map { item in
            guard let id = item[kSecAttrAccount as String] as? String,
                  let generic = item[kSecAttrGeneric as String] as? Data,
                  let data = item[kSecValueData as String] as? Data,
                  let info = try? JSONDecoder().decode(Info.self, from: generic)
            else { throw KeyVaultError.unreadable }
            return StoredKey(id: id, kind: info.kind, created: info.created, data: data)
        }
    }

    public func add(_ key: StoredKey) throws {
        var query = baseQuery()
        query[kSecAttrAccount as String] = key.id
        query[kSecAttrGeneric as String] = try JSONEncoder().encode(Info(kind: key.kind, created: key.created))
        query[kSecAttrAccessible as String] = key.kind == .secureEnclaveP256
            ? kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly : kSecAttrAccessibleAfterFirstUnlock
        query[kSecAttrSynchronizable as String] = false
        query[kSecValueData as String] = key.data
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else { throw KeyVaultError.keychain(status) }
    }

    public func remove(id: String) throws {
        var query = baseQuery()
        query[kSecAttrAccount as String] = id
        try delete(query)
    }

    public func removeAll() throws {
        try delete(baseQuery())
    }

    private func delete(_ query: [String: Any]) throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeyVaultError.keychain(status) }
    }
}
