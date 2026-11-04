import CapstanKit
import XCTest

@testable import CapstanAppCore

final class DecideTests: XCTestCase {
    private let host = ShellFixtures.hostA

    private func project() throws -> ProjectModel {
        try ShellFixtures.project(
            "p1",
            waiting: [try ShellFixtures.waiting(key: "k1", requestId: "req-k1", hash: ShellFixtures.hashA)],
            agents: [try ShellFixtures.agent(id: "a1", requestId: "req-a1", hash: ShellFixtures.hashB)],
            run: try ShellFixtures.run(hash: "cccccccccccc"))
    }

    func test_RequestTarget_forRow() throws {
        let found = RequestTarget.forRow(try project(), key: "k1")
        XCTAssertEqual(found?.requestId, "req-k1")
        XCTAssertEqual(found?.expect, ShellFixtures.hashA)
    }

    func test_RequestTarget_forRowAbsent() throws {
        XCTAssertNil(RequestTarget.forRow(try project(), key: "nope"))
    }

    func test_RequestTarget_forAgent() throws {
        let found = RequestTarget.forAgent(try project(), id: "a1")
        XCTAssertEqual(found?.requestId, "req-a1")
        XCTAssertEqual(found?.expect, ShellFixtures.hashB)
    }

    func test_RequestTarget_forAgentAbsent() throws {
        XCTAssertNil(RequestTarget.forAgent(try project(), id: "nope"))
    }

    func test_RequestTarget_forRun() throws {
        let found = RequestTarget.forRun(try project())
        XCTAssertEqual(found?.requestId, "run")
        XCTAssertEqual(found?.expect, "cccccccccccc")
    }

    func test_RequestTarget_forRunAbsent() throws {
        XCTAssertNil(RequestTarget.forRun(try ShellFixtures.project("p2")))
    }

    func test_LocalOutcome_notWaiting() {
        guard case .refused(let refusal, let hint) = LocalOutcome.notWaiting() else { return XCTFail("not refused") }
        XCTAssertEqual(refusal.code, .notWaiting)
        XCTAssertEqual(hint, .gone)
    }

    func test_LocalOutcome_busy() {
        guard case .refused(let refusal, let hint) = LocalOutcome.busy() else { return XCTFail("not refused") }
        XCTAssertEqual(refusal.code, .other("local-busy"))
        XCTAssertEqual(hint, .generic)
    }

    func test_BusyGate_present() {
        let key = BusyKey(hostId: host, projectId: "p1", requestId: "r1")
        XCTAssertEqual(BusyGate.check([key], key), LocalOutcome.busy())
    }

    func test_BusyGate_absent() {
        let key = BusyKey(hostId: host, projectId: "p1", requestId: "r1")
        XCTAssertNil(BusyGate.check([], key))
    }

    func test_BusyGate_sameRequestIdOtherProject() {
        let busy: Set<BusyKey> = [BusyKey(hostId: host, projectId: "p1", requestId: "run")]
        XCTAssertNil(BusyGate.check(busy, BusyKey(hostId: host, projectId: "p2", requestId: "run")))
    }

    func test_BusyTracker_beginTwice() {
        var tracker = BusyTracker()
        let key = BusyKey(hostId: host, projectId: "p1", requestId: "r1")
        XCTAssertTrue(tracker.begin(key))
        XCTAssertFalse(tracker.begin(key))
        XCTAssertEqual(tracker.keys, [key])
    }

    func test_BusyTracker_endRemoves() {
        var tracker = BusyTracker()
        let key = BusyKey(hostId: host, projectId: "p1", requestId: "r1")
        _ = tracker.begin(key)
        tracker.end(key)
        XCTAssertTrue(tracker.keys.isEmpty)
        XCTAssertTrue(tracker.begin(key))
    }

    func test_NoteText_spacesOnly() {
        XCTAssertNil(NoteText.normalized("   \n "))
        XCTAssertNil(NoteText.normalized(""))
    }

    func test_NoteText_padded() {
        XCTAssertEqual(NoteText.normalized("  not now \n"), "not now")
    }

    func test_DecideRouter_send() throws {
        let route = DecideRouter.route(
            .opApprove, target: .row(key: "k1"), project: try project(), busy: [], hostId: host, projectId: "p1",
            client: "Mac", nonce: "nonce-abcdef")
        guard case .send(let request, let key) = route else { return XCTFail("not sent") }
        XCTAssertEqual(request.requestId, "req-k1")
        XCTAssertEqual(request.expect, ShellFixtures.hashA)
        XCTAssertEqual(request.nonce, "nonce-abcdef")
        XCTAssertEqual(request.client, "Mac")
        XCTAssertEqual(request.hostId, host)
        XCTAssertEqual(request.projectId, "p1")
        XCTAssertEqual(request.verb, .opApprove)
        XCTAssertEqual(key, BusyKey(hostId: host, projectId: "p1", requestId: "req-k1"))
    }

    func test_DecideRouter_localNotWaiting() throws {
        let noProject = DecideRouter.route(
            .opApprove, target: .row(key: "k1"), project: nil, busy: [], hostId: host, projectId: "p1",
            client: "Mac", nonce: "nonce-abcdef")
        XCTAssertEqual(noProject, .local(LocalOutcome.notWaiting()))
        let noTarget = DecideRouter.route(
            .opApprove, target: .row(key: "missing"), project: try project(), busy: [], hostId: host,
            projectId: "p1", client: "Mac", nonce: "nonce-abcdef")
        XCTAssertEqual(noTarget, .local(LocalOutcome.notWaiting()))
    }

    func test_DecideRouter_localBusy() throws {
        let busy: Set<BusyKey> = [BusyKey(hostId: host, projectId: "p1", requestId: "req-k1")]
        let route = DecideRouter.route(
            .opApprove, target: .row(key: "k1"), project: try project(), busy: busy, hostId: host,
            projectId: "p1", client: "Mac", nonce: "nonce-abcdef")
        XCTAssertEqual(route, .local(LocalOutcome.busy()))
    }

    // MARK: Message, merge and plan routes

    /// A project with an active PM, an integration item and a plan that can be cancelled.
    private func projectWithPm(activePm: Bool = true) throws -> ProjectModel {
        var p = try ShellFixtures.project(
            "p1", waiting: [try ShellFixtures.waiting(key: "int1", kind: "integration", requestId: "req-int1", hash: ShellFixtures.hashA)])
        if activePm {
            let pm = try ShellFixtures.decode(
                StoredAgent.self,
                "{\"key\":\"agent:pm-1\",\"agentId\":\"pm-1\",\"kind\":\"PM\",\"role\":\"pm\",\"generation\":1,"
                    + "\"state\":\"active\",\"paused\":false,\"requestId\":\"agent:pm-1\",\"contentHash\":\"h\"}")
            p.agents[pm.key] = pm
        }
        let plan = try ShellFixtures.decode(
            StoredPlan.self,
            """
            {"key":"plan:x1","planId":"x1","sequence":1,"title":{"raw":"Plan","verified":true,"truncated":false},
             "state":"active","packages":{"total":0,"assigned":0,"reported":0,"integrated":0},
             "requestId":"plan:x1","contentHash":"dddddddddddd"}
            """)
        p.plans[plan.key] = plan
        return p
    }

    private func route(_ verb: DecideVerb, _ target: RouteTarget, _ project: ProjectModel) -> Route {
        DecideRouter.route(
            verb, target: target, project: project, busy: [], hostId: host, projectId: "p1", client: "Mac",
            nonce: "nonce-abcdef")
    }

    func test_DecideRouter_pmMessage() throws {
        guard case .send(let request, _) = route(.pmMessage("hello"), .pm, try projectWithPm()) else {
            return XCTFail("not sent")
        }
        XCTAssertEqual(request.requestId, PmBinding.requestId)
        XCTAssertEqual(request.expect, PmBinding.hash(agentId: "pm-1"))
        XCTAssertEqual(request.verb, .pmMessage("hello"))
    }

    func test_DecideRouter_pmMessageRefusedWithoutActivePm() throws {
        XCTAssertEqual(route(.pmMessage("hello"), .pm, try projectWithPm(activePm: false)), .local(LocalOutcome.noActivePm()))
    }

    func test_DecideRouter_integrationMergeWithAndWithoutInto() throws {
        let project = try projectWithPm()
        guard case .send(let plain, _) = route(.integrationMerge, .row(key: "int1"), project),
            case .send(let into, _) = route(.integrationMergeInto("release"), .row(key: "int1"), project)
        else { return XCTFail("not sent") }
        XCTAssertEqual(plain.requestId, "req-int1")
        XCTAssertEqual(plain.verb, .integrationMerge)
        XCTAssertEqual(into.requestId, "req-int1")
        XCTAssertEqual(into.verb, .integrationMergeInto("release"))
        XCTAssertEqual(into.expect, ShellFixtures.hashA)
    }

    func test_DecideRouter_integrationDiscard() throws {
        guard case .send(let request, _) = route(.integrationDiscard, .row(key: "int1"), try projectWithPm()) else {
            return XCTFail("not sent")
        }
        XCTAssertEqual(request.verb, .integrationDiscard)
        XCTAssertEqual(request.expect, ShellFixtures.hashA)
    }

    func test_DecideRouter_planCancel() throws {
        guard case .send(let request, _) = route(.planCancel, .plan(key: "plan:x1"), try projectWithPm()) else {
            return XCTFail("not sent")
        }
        XCTAssertEqual(request.requestId, "plan:x1")
        XCTAssertEqual(request.expect, "dddddddddddd")
        XCTAssertEqual(route(.planCancel, .plan(key: "plan:gone"), try projectWithPm()), .local(LocalOutcome.notWaiting()))
    }

    func test_ForwardRecording_forwarded() {
        let at = Date(timeIntervalSince1970: 1000)
        let result = ShellFixtures.result(outcome: .forwarded, requestId: "r9", hash: "dddddddddddd", messageId: "m1")
        let record = ForwardRecording.record(.forwarded(result), at: at)
        XCTAssertEqual(record, ForwardRecord(requestId: "r9", contentHash: "dddddddddddd", messageId: "m1", at: at))
    }

    func test_ForwardRecording_otherOutcomesNil() {
        let at = Date()
        XCTAssertNil(ForwardRecording.record(.applied(ShellFixtures.result(outcome: .applied)), at: at))
        XCTAssertNil(ForwardRecording.record(LocalOutcome.busy(), at: at))
        XCTAssertNil(ForwardRecording.record(.malformed("x", pendingId: nil), at: at))
        XCTAssertNil(ForwardRecording.record(.notSent(reason: .timeout, pendingId: UUID()), at: at))
    }

    func test_ForwardPill_sameHash() throws {
        let item = try ShellFixtures.waiting(key: "k1", hash: ShellFixtures.hashA)
        let record = ForwardRecord(requestId: "k1", contentHash: ShellFixtures.hashA, at: Date())
        XCTAssertTrue(ForwardPill.shown(item: item, record: record))
    }

    func test_ForwardPill_changedHash() throws {
        let item = try ShellFixtures.waiting(key: "k1", hash: ShellFixtures.hashB)
        let record = ForwardRecord(requestId: "k1", contentHash: ShellFixtures.hashA, at: Date())
        XCTAssertFalse(ForwardPill.shown(item: item, record: record))
        XCTAssertFalse(ForwardPill.shown(item: nil, record: record))
        XCTAssertFalse(ForwardPill.shown(item: item, record: nil))
    }
}
