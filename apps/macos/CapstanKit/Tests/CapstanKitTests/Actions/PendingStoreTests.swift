import XCTest
@testable import CapstanKit

final class PendingStoreTests: XCTestCase {
    func testSurvivesReload() async {
        let url = tempStoreURL()
        let record = PendingDecision(request: makeRequest(verb: .designChanges(askedUser: "a", link: nil)), sentAt: Date(timeIntervalSince1970: 1_000))
        let store = PendingStore(url: url)
        await store.add(record)
        let reloaded = PendingStore(url: url)
        let all = await reloaded.all()
        XCTAssertEqual(all, [record])
    }

    func testRemoveByIdAndByHost() async {
        let url = tempStoreURL()
        let host = UUID()
        let a = PendingDecision(request: makeRequest(verb: .opApprove, host: host), sentAt: Date(timeIntervalSince1970: 1))
        let b = PendingDecision(request: makeRequest(verb: .opApprove, host: host), sentAt: Date(timeIntervalSince1970: 2))
        let c = PendingDecision(request: makeRequest(verb: .opApprove), sentAt: Date(timeIntervalSince1970: 3))
        let store = PendingStore(url: url)
        for record in [a, b, c] { await store.add(record) }
        await store.remove(a.id)
        var ids = await store.all().map(\.id)
        XCTAssertEqual(ids, [b.id, c.id])
        await store.remove(hostId: host)
        ids = await PendingStore(url: url).all().map(\.id)
        XCTAssertEqual(ids, [c.id])
    }

    func testCorruptFileIsEmpty() async throws {
        let url = tempStoreURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("{not json".utf8).write(to: url)
        let store = PendingStore(url: url)
        let all = await store.all()
        XCTAssertTrue(all.isEmpty)
        await store.add(PendingDecision(request: makeRequest(verb: .opApprove), sentAt: Date()))
        let count = await PendingStore(url: url).all().count
        XCTAssertEqual(count, 1)
    }
}
