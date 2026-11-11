import XCTest
@testable import CapstanKit

final class WorkspaceItemsTests: XCTestCase {
    private func docBlocks() throws -> [String] {
        try ProtocolDoc.jsonBlocks().filter { $0.section == 5 }.map(\.text)
    }

    private func json(_ text: String) throws -> JSONValue { try decodeJSON(Data(text.utf8)) }

    private func setSample() throws -> IntegrationSet {
        let value = try json(try docBlocks()[1])
        return try XCTUnwrap(IntegrationSet(wire: try XCTUnwrap(value["set"])))
    }

    // MARK: the documented samples

    func testTheDocumentedSetDecodesInMergeOrder() throws {
        let set = try setSample()
        XCTAssertEqual(set.setId, "set-4")
        XCTAssertEqual(set.state, "running")
        XCTAssertEqual(set.repo, "web")
        XCTAssertEqual(set.position, 1)
        XCTAssertEqual(set.attempt, 2)
        XCTAssertFalse(set.reviewSubject)
        XCTAssertEqual(set.members.map(\.repo), ["api", "web", "worker", "infra"])
        XCTAssertEqual(set.members.map(\.position), [0, 1, 2, 3])
        XCTAssertEqual(set.members.map(\.state), ["confirmed", "merged", "conflicted", "discarded"])
        XCTAssertEqual(set.members[1].attempt, 2)
        XCTAssertTrue(set.members[1].isRedone)
        XCTAssertNil(set.members[2].headSha)
        XCTAssertEqual(set.member?.repo, "web")
    }

    func testTheDroppedMemberIsOutsideTheCounts() throws {
        let set = try setSample()
        XCTAssertEqual(set.droppedCount, 1)
        XCTAssertEqual(set.liveMembers.map(\.repo), ["api", "web", "worker"])
        XCTAssertEqual(set.inMainCount, 1)
        XCTAssertEqual(set.builtCount, 2)
        XCTAssertFalse(set.isInMain)
        XCTAssertEqual(set.next?.repo, "web")
    }

    func testOnlyTheMemberBeforeItBlocksAMerge() throws {
        let set = try setSample()
        XCTAssertNil(set.blocker(of: "api"))
        XCTAssertNil(set.blocker(of: "web"))
        XCTAssertEqual(set.blocker(of: "worker")?.repo, "web")
        // A dropped member never blocks, and an unknown repo has no blocker.
        XCTAssertNil(set.blocker(of: "nowhere"))
    }

    func testMembersAreSortedByPositionAndCappedAt32() throws {
        var rows: [String] = []
        for index in (0..<40).reversed() {
            rows.append("{\"repo\":\"r\(index)\",\"position\":\(index),\"state\":\"running\"}")
        }
        let value = try json(
            "{\"setId\":\"s\",\"state\":\"running\",\"repo\":\"r0\",\"position\":0,\"members\":[\(rows.joined(separator: ","))]}")
        let set = try XCTUnwrap(IntegrationSet(wire: value))
        XCTAssertEqual(set.members.count, 32)
        XCTAssertEqual(set.members.first?.repo, "r0")
        XCTAssertEqual(set.members.last?.repo, "r31")
    }

    func testAMemberThatDoesNotReadIsDropped() throws {
        let value = try json(
            "{\"setId\":\"s\",\"state\":\"merged\",\"repo\":\"a\",\"position\":0,\"members\":[{\"repo\":\"a\",\"position\":0,\"state\":\"merged\",\"headSha\":\"NOTHEX\"},{\"position\":1},\"x\"]}")
        let set = try XCTUnwrap(IntegrationSet(wire: value))
        XCTAssertEqual(set.members.count, 1)
        XCTAssertNil(set.members[0].headSha)
        XCTAssertEqual(set.members[0].attempt, 1)
        XCTAssertFalse(set.members[0].dropped)
    }

    func testASetWithoutItsKeyFieldsReadsAsAbsent() throws {
        XCTAssertNil(IntegrationSet(wire: try json("{\"setId\":\"s\"}")))
        XCTAssertNil(IntegrationSet(wire: .string("set")))
    }

    func testTheDocumentedVerifyByRepoDecodes() throws {
        let value = try json(try docBlocks()[2])
        let rows = try XCTUnwrap(RepoVerify.list(in: value["verify"]))
        XCTAssertEqual(rows.map(\.repo), ["api", "web", "worker"])
        XCTAssertEqual(rows[0].latest?.state, "passed")
        XCTAssertEqual(rows[1].latest?.state, "failed")
        XCTAssertEqual(rows[1].lastPassed?.commitSha, "51ab07e3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9")
        XCTAssertNil(rows[2].latest)
        XCTAssertNil(rows[2].lastPassed)
    }

    func testTheDocumentedReportEventCarriesItsCommits() throws {
        let line = try docBlocks()[3]
        guard case .event(_, let event) = try LineDecoder().decode(Data(line.utf8)) else { return XCTFail("event") }
        let commits = try XCTUnwrap(event.body.reportCommits)
        XCTAssertEqual(commits.map(\.repo), ["api", "web"])
        XCTAssertEqual(commits[0].branch, "feat/PM-512-refund-sync")
        XCTAssertEqual(commits[1].baseSha, "89abcdef0123456789abcdef0123456789abcdef")
        XCTAssertNil(event.body.integrationSet)
    }

    func testTheDocumentedIntegrationFinishedEventCarriesItsSet() throws {
        let line = try docBlocks()[4]
        guard case .event(_, let event) = try LineDecoder().decode(Data(line.utf8)) else { return XCTFail("event") }
        let set = try XCTUnwrap(event.body.integrationSet)
        XCTAssertEqual(set, try setSample())
        XCTAssertNil(event.body.reportCommits)
    }

    func testTheDocumentedStatusAgentCarriesWorktrees() throws {
        let status = try json(try docBlocks()[5])
        let agent = try XCTUnwrap(status["agents"]?[0])
        let worktrees = try XCTUnwrap(AgentWorktree.list(in: agent))
        XCTAssertEqual(worktrees.map(\.repo), ["api", "web"])
        XCTAssertEqual(worktrees[0].path, "/work/cf/dev-4/api")
        XCTAssertEqual(agent["changeFolder"], .string("/work/cf/dev-4"))
    }

    // MARK: items

    private func waiting(_ extra: String) throws -> WaitingItem {
        try WaitingItem(json: try json(
            "{\"key\":\"integration:1\",\"requestId\":\"integration:i\",\"kind\":\"integration\",\"since\":\"s\","
                + "\"summary\":\"merged b (repo web of set set-4)\",\"contentHash\":\"h\",\"needsTerminal\":false,"
                + "\"integrationState\":\"merged\",\"branch\":\"b\",\"headSha\":\"3f2a9c1d5b7e\"\(extra)}"))
    }

    private var setText: String {
        "{\"setId\":\"set-4\",\"state\":\"running\",\"repo\":\"web\",\"position\":1,\"attempt\":1,\"reviewSubject\":true,"
            + "\"dropped\":false,\"members\":[{\"repo\":\"api\",\"position\":0,\"state\":\"confirmed\"},"
            + "{\"repo\":\"web\",\"position\":1,\"state\":\"merged\"}]}"
    }

    func testAnOlderHostsItemHasNoSet() throws {
        let item = try waiting("")
        XCTAssertNil(item.set)
        XCTAssertNil(item.integrationDetails)
    }

    func testTheSetOfAnItemAndOfItsDetailsBothRead() throws {
        let onItem = try waiting(",\"set\":\(setText)")
        XCTAssertEqual(onItem.set?.setId, "set-4")
        XCTAssertEqual(onItem.set?.reviewSubject, true)
        let inDetails = try waiting(",\"integrationDetails\":{\"recorded\":false,\"set\":\(setText)}")
        XCTAssertEqual(inDetails.set?.setId, "set-4")
        XCTAssertEqual(inDetails.integrationDetails?.set?.members.count, 2)
    }

    func testByRepoReadsWithoutAVerifyResultOfItsOwn() throws {
        let item = try waiting(
            ",\"integrationDetails\":{\"recorded\":true,\"verify\":{\"byRepo\":[{\"repo\":\"api\",\"latest\":null,\"lastPassed\":null}]}}")
        let details = try XCTUnwrap(item.integrationDetails)
        XCTAssertNil(details.verify)
        XCTAssertEqual(details.byRepo, [RepoVerify(repo: "api")])
    }

    func testAWrongTypedMemberReadsAsAbsent() throws {
        let item = try waiting(",\"set\":\"nope\",\"integrationDetails\":{\"recorded\":true,\"set\":5,\"verify\":{\"byRepo\":3}}")
        XCTAssertNil(item.set)
        XCTAssertNil(item.integrationDetails?.set)
        XCTAssertNil(item.integrationDetails?.byRepo)
    }

    func testTheStoredFormKeepsTheSetAndByRepo() throws {
        let item = try waiting(
            ",\"set\":\(setText),\"integrationDetails\":{\"recorded\":true,\"set\":\(setText),"
                + "\"verify\":{\"latest\":{\"state\":\"passed\",\"commitSha\":\"3f2a9c1d5b7e\",\"finishedAt\":\"t\"},"
                + "\"byRepo\":[{\"repo\":\"api\",\"latest\":{\"state\":\"passed\",\"commitSha\":\"3f2a9c1d5b7e\",\"finishedAt\":\"t\"}}]}}")
        let back = try WaitingItem(json: StoredWaiting(item).json)
        XCTAssertEqual(back.set, item.set)
        XCTAssertEqual(back.integrationDetails, item.integrationDetails)
        XCTAssertEqual(back.integrationDetails?.byRepo?.first?.latest?.state, "passed")
        XCTAssertEqual(back.integrationDetails?.verify?.latest.state, "passed")
    }

    func testAnOlderSavedFileDecodesWithoutTheNewMembers() throws {
        let old = StoredWaiting(
            key: "k", requestId: "r", kind: .integration, since: "s", summary: UntrustedText(raw: "x", verified: true, truncated: false),
            contentHash: "h", needsTerminal: false)
        var stripped = try JSONEncoder().encode(old)
        // The encoder writes `set` only when it is set; none was set, so the bytes are what an older build wrote.
        stripped = try JSONSerialization.data(withJSONObject: JSONSerialization.jsonObject(with: stripped))
        XCTAssertNil(try JSONDecoder().decode(StoredWaiting.self, from: stripped).set)
    }

    func testAnAgentItemReadsItsWorktrees() throws {
        let base =
            "{\"key\":\"agent:dev-4\",\"agentId\":\"dev-4\",\"kind\":\"Developer\",\"role\":\"dev\",\"generation\":1,"
            + "\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:dev-4\",\"contentHash\":\"h\""
        let plain = try AgentItem(json: try json(base + "}"))
        XCTAssertNil(plain.worktrees)
        XCTAssertNil(plain.changeFolder)
        let held = try AgentItem(json: try json(
            base + ",\"worktrees\":[{\"repo\":\"api\",\"path\":\"/w/api\",\"branch\":\"b\"},{\"repo\":1}],\"changeFolder\":\"/w\"}"))
        XCTAssertEqual(held.worktrees, [AgentWorktree(repo: "api", path: "/w/api", branch: "b")])
        XCTAssertEqual(held.changeFolder, "/w")
        let stored = try AgentItem(json: StoredAgent(held).json)
        XCTAssertEqual(stored.worktrees, held.worktrees)
        XCTAssertEqual(stored.changeFolder, "/w")
    }
}
