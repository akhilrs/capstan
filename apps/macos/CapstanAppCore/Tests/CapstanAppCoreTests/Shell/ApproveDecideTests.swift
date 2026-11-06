import CapstanKit
import XCTest

@testable import CapstanAppCore

/// Routing and words of `op approve-as` in the shell.
final class ApproveDecideTests: XCTestCase {
    private let host = ShellFixtures.hostA

    private func project() throws -> ProjectModel {
        try ShellFixtures.project(
            "p1", waiting: [try ShellFixtures.waiting(key: "k1", requestId: "req-k1", hash: ShellFixtures.hashA)])
    }

    private func route(_ choice: ApproveChoice, busy: Set<BusyKey> = []) throws -> Route {
        DecideRouter.route(
            .opApproveAs(choice), target: .row(key: "k1"), project: try project(), busy: busy, hostId: host,
            projectId: "p1", client: "Mac", nonce: "nonce-abcdef")
    }

    func test_route_everyChoiceCarriesTheRowBinding() throws {
        let choices: [ApproveChoice] = [.once, .exact, .prefix(raw: "git commit"), .autoHour, .autoSession]
        let words = [["approve-as", "once"], ["approve-as", "exact"], ["approve-as", "prefix", "--prefix", "git commit"],
                     ["approve-as", "auto-hour"], ["approve-as", "auto-session"]]
        for (choice, expected) in zip(choices, words) {
            guard case .send(let request, _) = try route(choice) else { return XCTFail("not sent") }
            XCTAssertEqual(request.requestId, "req-k1")
            XCTAssertEqual(request.expect, ShellFixtures.hashA)
            XCTAssertTrue(try request.remoteArguments().joined(separator: "\u{1}").hasSuffix(expected.joined(separator: "\u{1}")))
        }
    }

    func test_route_prefixIsSentWithoutCleaning() throws {
        let raw = "git  commit "
        guard case .send(let request, _) = try route(.prefix(raw: raw)) else { return XCTFail("not sent") }
        XCTAssertEqual(try request.remoteArguments().last, raw)
    }

    func test_route_aSecondDecisionForTheRequestIsBusy() throws {
        let key = BusyKey(hostId: host, projectId: "p1", requestId: "req-k1")
        guard case .local(let outcome) = try route(.once, busy: [key]) else { return XCTFail("sent") }
        XCTAssertEqual(outcome, LocalOutcome.busy())
    }

    func test_route_rowGone_isNotWaiting() throws {
        let route = DecideRouter.route(
            .opApproveAs(.once), target: .row(key: "gone"), project: try project(), busy: [], hostId: host,
            projectId: "p1", client: "Mac", nonce: "nonce-abcdef")
        guard case .local(let outcome) = route else { return XCTFail("sent") }
        XCTAssertEqual(outcome, LocalOutcome.notWaiting())
    }
}
