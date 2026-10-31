import XCTest
@testable import CapstanKit

final class MergeVerbTests: XCTestCase {
    func testArgumentsAndRow() throws {
        let request = makeRequest(verb: .integrationMerge, requestId: "integration:int-7")
        XCTAssertEqual(
            try request.remoteArguments(),
            ["decide", "--project", "proj-1", "--json", "integration:int-7", "--expect", testHash, "--nonce", testNonce,
             "--client", "Mac", "merge"])
        XCTAssertEqual(DecideVerb.integrationMerge.row, DecideVerbRow(kind: "integration", verb: "merge", outcome: "applied"))
        XCTAssertTrue(DecideVerb.integrationMerge.isDestructive, "a merge confirms first")
        XCTAssertTrue(DecideVerbRow.documented.contains(DecideVerb.integrationMerge.row))
    }

    func testAMergeMayRunForMinutes() {
        XCTAssertEqual(Decider.timeout(for: .integrationMerge), Decider.mergeTimeoutSeconds)
        XCTAssertGreaterThan(Decider.mergeTimeoutSeconds, 300, "longer than cstan decide waits")
        XCTAssertEqual(Decider.timeout(for: .opApprove), Decider.timeoutSeconds)
    }

    func testTheResultCarriesTheMerge() throws {
        let line = """
            {"v":1,"decisionId":"remote-9","requestId":"integration:int-7","nonce":"\(testNonce)","outcome":"applied",\
            "repeat":false,"kind":"integration","verb":"merge","contentHash":"\(testHash)","eventSeq":12,\
            "merge":{"integrationId":"int-7","branch":"integration/plan-3","mergeCommit":"9d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e",\
            "alreadyInHead":false,"verify":{"state":"passed","runId":"verify-12"},"cleanup":{"state":"scheduled"},\
            "push":{"state":"disabled","reason":"push_off"}}}
            """
        guard case .result(let result) = try DecideAnswer(data: Data(line.utf8)) else { return XCTFail("not a result") }
        let merge = try XCTUnwrap(result.merge)
        XCTAssertEqual(merge.integrationId, "int-7")
        XCTAssertEqual(merge.branch, "integration/plan-3")
        XCTAssertEqual(merge.mergeCommit, "9d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e")
        XCTAssertFalse(merge.alreadyInHead)
        XCTAssertEqual(merge.verify, .passed(runId: "verify-12"))
        XCTAssertTrue(merge.cleanupScheduled)
        XCTAssertTrue(merge.pushDisabled)
    }

    func testAMergeOfAHeadAlreadyInTheBranch() throws {
        let json = try decodeJSON(Data("""
            {"integrationId":"int-8","branch":"integration/plan-4","mergeCommit":null,"alreadyInHead":true,\
            "verify":{"state":"not_configured"},"cleanup":null,"push":{"state":"disabled","reason":"push_off"}}
            """.utf8))
        let merge = try MergeResult(json: json)
        XCTAssertNil(merge.mergeCommit)
        XCTAssertTrue(merge.alreadyInHead)
        XCTAssertEqual(merge.verify, .notConfigured)
        XCTAssertFalse(merge.cleanupScheduled)
    }

    func testOtherResultsHaveNoMerge() throws {
        guard case .result(let result) = try DecideAnswer(data: Data(resultLine().utf8)) else { return XCTFail() }
        XCTAssertNil(result.merge)
    }

    func testMergeRefusalsDecodeAsTheirCode() throws {
        for code in MergeResult.refusalCodes {
            guard case .refusal(let refusal) = try DecideAnswer(data: Data(refusalLine(code).utf8)) else {
                return XCTFail(code)
            }
            XCTAssertEqual(refusal.code, .other(code))
        }
    }

    func testOnlyABuiltIntegrationCanBeMerged() throws {
        let caps = HostCapabilities(reachable: true, events: true, decide: true)
        func integration(_ state: String) throws -> WaitingItem {
            try WaitingItem(json: .object([
                "key": .string("integration:7"), "requestId": .string("integration:int-7"), "kind": .string("integration"),
                "since": .string("t"), "summary": .string(state + " integration/plan-3"), "contentHash": .string(testHash),
                "needsTerminal": .bool(false), "integrationState": .string(state),
            ]))
        }
        XCTAssertEqual(availability(.integrationMerge, capabilities: caps, item: try integration("merged"), option: nil), .available)
        if case .available = availability(.integrationMerge, capabilities: caps, item: try integration("conflicted"), option: nil) {
            XCTFail("a conflicted integration has nothing to merge")
        }
        if case .available = availability(.integrationMerge, capabilities: caps, item: try waitingItem(kind: "op"), option: nil) {
            XCTFail("an op is not merged")
        }
    }
}
