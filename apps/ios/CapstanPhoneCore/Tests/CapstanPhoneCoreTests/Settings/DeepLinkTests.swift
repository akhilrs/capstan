import Foundation
import XCTest
@testable import CapstanPhoneCore

final class DeepLinkTests: XCTestCase {
    private let host = UUID(uuidString: "E621E1F8-C36C-495A-93FC-0C247A3E6E5F")!

    func testRoundTrip() {
        let links: [DeepLink] = [
            .item(hostId: host, projectId: "proj-1", requestId: "op:proposal-2"),
            .agent(hostId: host, projectId: "proj-1", agentId: "reviewer-2"),
            .plan(hostId: host, projectId: "proj-1", planId: "plan-3"),
            .conversation(hostId: host, projectId: "proj_1"),
        ]
        for link in links {
            let url = link.url
            XCTAssertNotNil(url, "\(link)")
            XCTAssertEqual(url.flatMap(DeepLink.init(url:)), link)
            XCTAssertEqual(DeepLink(text: link.text), link)
        }
    }

    func testFormat() {
        XCTAssertEqual(
            DeepLink.item(hostId: host, projectId: "p1", requestId: "message:msg-1").text,
            "capstan://item/e621e1f8-c36c-495a-93fc-0c247a3e6e5f/p1/message:msg-1")
        XCTAssertEqual(
            DeepLink.conversation(hostId: host, projectId: "p1").text,
            "capstan://conversation/e621e1f8-c36c-495a-93fc-0c247a3e6e5f/p1")
    }

    func testMalformedIsRejected() {
        let h = "e621e1f8-c36c-495a-93fc-0c247a3e6e5f"
        let bad = [
            "https://item/\(h)/p1/op:1",
            "capstan:item/\(h)/p1/op:1",
            "capstan://item/not-a-uuid/p1/op:1",
            "capstan://item/\(h)/p1",
            "capstan://item/\(h)/p1/op:1/extra",
            "capstan://conversation/\(h)/p1/op:1",
            "capstan://unknown/\(h)/p1/op:1",
            "capstan://item/\(h)//op:1",
            "capstan://item/\(h)/-p1/op:1",
            "capstan://item/\(h)/.p1/op:1",
            "capstan://item/\(h)/p1/op%3A1",
            "capstan://item/\(h)/p1/op:1?x=1",
            "capstan://item/\(h)/p1/op:1#frag",
            "capstan://item/\(h)/p 1/op:1",
            "capstan://item/\(h)/p1/<script>",
            "capstan://item/\(h)/p1/ölçek",
            "capstan://item/\(h)/p1/op:1/",
            "capstan://",
            "",
        ]
        for text in bad { XCTAssertNil(DeepLink(text: text), text) }
    }

    func testOversizeIsRejected() {
        let long = String(repeating: "a", count: DeepLink.maxIdBytes + 1)
        XCTAssertNil(DeepLink(text: "capstan://item/\(host.uuidString)/\(long)/op:1"))
        XCTAssertNil(DeepLink.item(hostId: host, projectId: long, requestId: "op:1").url)
        let fits = String(repeating: "a", count: DeepLink.maxIdBytes)
        XCTAssertNotNil(DeepLink.item(hostId: host, projectId: fits, requestId: "op:1").url)
    }

    func testOnlyIdsAreInTheURL() {
        // Host text such as a summary can never be put in a link: it does not pass as an id.
        let summary = "Run rm -rf / now"
        XCTAssertNil(DeepLink.item(hostId: host, projectId: "p1", requestId: summary).url)
        XCTAssertFalse(DeepLink.isId(summary))
    }
}
