import Foundation
import XCTest
@testable import CapstanKit

final class MergeTargetsCallTests: XCTestCase {
    func testTheArgumentsNameTheProjectAndAskForJSON() throws {
        XCTAssertEqual(
            try MergeTargetsCall(projectId: "ims-api").remoteArguments(),
            ["merge-targets", "--project", "ims-api", "--json"])
        XCTAssertThrowsError(try MergeTargetsCall(projectId: "-x").remoteArguments())
    }

    func testAListIsReadWithItsDefaultAndCheckout() {
        let json = #"{"enabled":true,"defaultBranch":"main","checkedOut":"develop","branches":["develop","main","release/1"]}"#
        let result = MergeTargetsCall.result(stdout: Data((json + "\n").utf8), stderr: "", exitCode: 0)
        guard case .targets(let targets) = result else { return XCTFail("\(result)") }
        XCTAssertTrue(targets.enabled)
        XCTAssertEqual(targets.defaultBranch, "main")
        XCTAssertEqual(targets.checkedOut, "develop")
        XCTAssertEqual(targets.branches, ["develop", "main", "release/1"])
        XCTAssertEqual(targets.preferred, "main")
    }

    func testAnOlderCstanIsUnsupportedAndARefusalIsAnError() {
        XCTAssertEqual(
            MergeTargetsCall.result(stdout: Data(), stderr: "cstan: unknown command: merge-targets\n", exitCode: 2),
            .unsupported)
        let refusal = #"{"v":1,"error":{"code":"not_answering","message":"the controller does not answer"}}"#
        XCTAssertEqual(
            MergeTargetsCall.result(stdout: Data(refusal.utf8), stderr: "", exitCode: 1),
            .error("the controller does not answer"))
    }

    func testMergeIntoSendsTheBranchAsTheValueOfInto() throws {
        let verb = DecideVerb.integrationMergeInto("develop")
        XCTAssertTrue(verb.isMerge)
        XCTAssertTrue(verb.isDestructive)
        XCTAssertEqual(verb.row, DecideVerb.integrationMerge.row)
        XCTAssertEqual(Decider.timeout(for: verb), Decider.mergeTimeoutSeconds)
    }
}
