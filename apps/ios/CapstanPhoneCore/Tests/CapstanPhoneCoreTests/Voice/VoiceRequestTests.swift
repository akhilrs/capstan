import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

final class VoiceRequestTests: XCTestCase {
    typealias F = VoiceFixture
    private let nonce = "AbCdEfGh12345678"

    private func make(_ choice: VoiceChoice, _ item: StoredWaiting = F.item(), _ settings: PhoneSettings = F.settings()) throws -> DecideRequest {
        try VoiceRequest.make(choice: choice, target: F.target(item), settings: settings, nonce: nonce)
    }

    func testEachVerb() throws {
        XCTAssertEqual(try make(.once).verb, .opApproveAs(.once))
        XCTAssertEqual(try make(.exact).verb, .opApproveAs(.exact))
        XCTAssertEqual(try make(.prefix(raw: "git push", number: 1)).verb, .opApproveAs(.prefix(raw: "git push")))
        XCTAssertEqual(try make(.autoHour).verb, .opApproveAs(.autoHour))
        XCTAssertEqual(try make(.autoSession).verb, .opApproveAs(.autoSession))
        XCTAssertEqual(try make(.deny).verb, .opDeny(note: nil))
        XCTAssertEqual(try make(.forwardedApprove, F.item(choices: nil)).verb, .opApprove)
    }

    func testTheRequestCarriesTheReadBackHashAndTarget() throws {
        let request = try make(.once, F.item(hash: "0123456789ab"))
        XCTAssertEqual(request.expect, "0123456789ab")
        XCTAssertEqual(request.hostId, F.hostA)
        XCTAssertEqual(request.projectId, "p1")
        XCTAssertEqual(request.requestId, "op:k1")
        XCTAssertEqual(request.nonce, nonce)
        XCTAssertEqual(request.client, "iPhone voice")
        XCTAssertNoThrow(try request.remoteArguments())
    }

    func testOnlyListedChoicesAreBuilt() {
        XCTAssertThrowsError(try make(.exact, F.item(), F.settings(grants: false)))
        XCTAssertThrowsError(try make(.once, F.item(), F.settings(decisions: false)))
        XCTAssertThrowsError(try make(.deny, F.item(), F.settings(decisions: false)))
        XCTAssertThrowsError(try make(.prefix(raw: "rm", number: 1)))
        XCTAssertThrowsError(try make(.once, F.item(choices: nil)))
        XCTAssertThrowsError(try make(.forwardedApprove))
        let floor = ApproveChoices(once: true, exact: true, limit: .floor)
        XCTAssertThrowsError(try make(.once, F.item(choices: floor)))
        XCTAssertNoThrow(try make(.deny, F.item(choices: floor)))
    }

    func testMultiByteClientLabelIsCutAtACharacterBoundary() throws {
        let label = String(repeating: "é", count: 40)
        let client = VoiceRequest.client(label)
        XCTAssertTrue(client.hasSuffix(" voice"))
        XCTAssertLessThanOrEqual(client.utf8.count, 64)
        XCTAssertNotNil(String(data: Data(client.utf8), encoding: .utf8))
        XCTAssertEqual(client, String(repeating: "é", count: 29) + " voice")
        let request = try make(.once, F.item(), F.settings(label: label))
        XCTAssertEqual(request.client, client)
    }

    func testShortAndExactLabels() {
        XCTAssertEqual(VoiceRequest.client("Akhil iPhone"), "Akhil iPhone voice")
        let exact = String(repeating: "a", count: 58)
        XCTAssertEqual(VoiceRequest.client(exact).utf8.count, 64)
        XCTAssertEqual(VoiceRequest.client(exact + "a").utf8.count, 64)
    }
}
