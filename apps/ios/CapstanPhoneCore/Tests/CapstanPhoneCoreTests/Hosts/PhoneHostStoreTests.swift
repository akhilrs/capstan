import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class PhoneHostStoreTests: XCTestCase {
    func testSaveThenLoadReturnsTheSameHosts() throws {
        let store = PhoneHostStore(directory: try makeTempDirectory(self))
        let hosts = [
            PhoneHost(id: UUID(), hostname: "box", user: "dev"),
            PhoneHost(id: UUID(), hostname: "fe80::1%en0", port: 2222, user: "ops"),
        ]
        try store.save(hosts)
        XCTAssertEqual(store.load(), hosts)
    }

    func testMissingFileGivesNoHosts() throws {
        XCTAssertEqual(PhoneHostStore(directory: try makeTempDirectory(self)).load(), [])
    }

    func testCorruptFileGivesNoHostsAndIsKeptUntilTheNextSave() throws {
        let directory = try makeTempDirectory(self)
        let file = directory.appendingPathComponent("phone-hosts.json")
        let garbage = Data("[{\"id\":".utf8)
        try garbage.write(to: file)

        let store = PhoneHostStore(directory: directory)
        XCTAssertEqual(store.load(), [])
        XCTAssertEqual(try Data(contentsOf: file), garbage)

        let host = PhoneHost(id: UUID(), hostname: "box", user: "dev")
        try store.save([host])
        XCTAssertEqual(store.load(), [host])
    }
}
