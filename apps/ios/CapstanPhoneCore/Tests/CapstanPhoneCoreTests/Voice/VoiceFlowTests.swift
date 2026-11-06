import CapstanKit
import Foundation
import XCTest
@testable import CapstanPhoneCore

/// A scripted environment that records the order of everything the flow does.
final class FakeVoiceEnv: VoiceEnvironment, @unchecked Sendable {
    private let lock = NSLock()
    private var log: [String] = []
    private var insideHosts = 0
    private var confirmAnswers: [Bool]
    private var chooseAnswers: [Int?]
    private let lease = SessionLeaseBox()

    var ready: VoiceReadiness = .ready
    var current: PhoneSettings
    var read: VoiceRead
    var outcome: DecideOutcome = VoiceFixture.applied(verb: "approve-as", approval: ApprovalResult(choice: "once"))
    var hostError: VoiceEnvironmentError?
    var background = false
    var isBackground: Bool { background }
    var sent: [DecideRequest] = []
    var sentOutside = false
    var confirmTexts: [String] = []
    var prompts: [VoicePrompt] = []

    init(
        models: [VoiceHostModel], settings: PhoneSettings = VoiceFixture.settings(), confirms: [Bool] = [true],
        chooses: [Int?] = []
    ) {
        read = VoiceRead(models: models, now: VoiceFixture.now)
        current = settings
        confirmAnswers = confirms
        chooseAnswers = chooses
    }

    var events: [String] { lock.lock(); defer { lock.unlock() }; return log }
    func note(_ text: String) { lock.lock(); log.append(text); lock.unlock() }
    func count(_ text: String) -> Int { events.filter { $0 == text }.count }

    func readiness() async -> VoiceReadiness { note("readiness"); return ready }
    func settings() async -> PhoneSettings { current }
    func nonce() -> String { "AbCdEfGh12345678" }

    func withHosts<T: Sendable>(
        budget: TimeInterval, _ body: @Sendable (VoiceRead) async throws -> T
    ) async throws -> T {
        if let hostError { throw hostError }
        return try await lease.hold(
            isBackground: { self.isBackground },
            start: { self.note("lease.start") },
            stop: { self.note("lease.stop") },
            body: {
                self.lock.lock(); self.insideHosts += 1; self.lock.unlock()
                defer { self.lock.lock(); self.insideHosts -= 1; self.lock.unlock() }
                self.note("hosts.begin")
                let value = try await body(self.read)
                self.note("hosts.end")
                return value
            })
    }

    func confirm(_ text: String) async -> Bool {
        lock.lock(); defer { lock.unlock() }
        log.append("confirm")
        confirmTexts.append(text)
        return confirmAnswers.isEmpty ? false : confirmAnswers.removeFirst()
    }

    func choose(_ prompt: VoicePrompt) async -> Int? {
        lock.lock(); defer { lock.unlock() }
        log.append("choose")
        prompts.append(prompt)
        return chooseAnswers.isEmpty ? nil : chooseAnswers.removeFirst()
    }

    func voiceSend(_ request: DecideRequest) async -> DecideOutcome {
        lock.lock(); defer { lock.unlock() }
        log.append("send")
        if insideHosts == 0 { sentOutside = true }
        sent.append(request)
        return outcome
    }
}

final class VoiceFlowTests: XCTestCase {
    typealias F = VoiceFixture

    private func model(_ item: StoredWaiting = F.item()) -> [VoiceHostModel] { [F.host(items: [item])] }

    func testReadinessComesFirstAndNothingElseRuns() async {
        let cases: [(VoiceReadiness, String)] = [
            (.keysUnreadable, "refusal.locked"), (.notPaired, "refusal.notPaired"), (.noHosts, "refusal.notPaired"),
        ]
        for (readiness, key) in cases {
            for action in [VoiceAction.list, .approveOnce, .approve, .deny] {
                let env = FakeVoiceEnv(models: model())
                env.ready = readiness
                let lines = await VoiceFlow.run(action, env)
                XCTAssertEqual(lines, [VoiceTexts.text(key)])
                XCTAssertEqual(env.events, ["readiness"])
                XCTAssertTrue(env.sent.isEmpty)
            }
        }
    }

    func testListNeedsNoSettingAndSendsNothing() async {
        let env = FakeVoiceEnv(models: model(), settings: F.settings(decisions: false, grants: false))
        let lines = await VoiceFlow.run(.list, env)
        XCTAssertEqual(lines.first, "1 item waits in Capstan.")
        XCTAssertTrue(env.sent.isEmpty)
        XCTAssertEqual(env.count("confirm"), 0)
    }

    func testApproveOnceOrderAndSingleSend() async {
        let env = FakeVoiceEnv(models: model())
        let lines = await VoiceFlow.run(.approveOnce, env)
        XCTAssertEqual(env.events, ["readiness", "lease.start", "hosts.begin", "confirm", "send", "hosts.end"])
        XCTAssertEqual(env.sent.count, 1)
        XCTAssertFalse(env.sentOutside)
        XCTAssertEqual(env.sent[0].verb, .opApproveAs(.once))
        XCTAssertEqual(env.sent[0].expect, F.hash)
        XCTAssertEqual(lines, ["Approved once in payments-api."])
        XCTAssertEqual(
            env.confirmTexts,
            ["Approve once in payments-api. The command is: git push origin main. End of command. Approve it?"])
    }

    func testNoOrCancelSendsNothing() async {
        for action in [VoiceAction.approveOnce, .deny] {
            let env = FakeVoiceEnv(models: model(), confirms: [false])
            let lines = await VoiceFlow.run(action, env)
            XCTAssertTrue(env.sent.isEmpty)
            XCTAssertEqual(lines, ["Nothing was sent."])
            XCTAssertEqual(env.count("confirm"), 1)
        }
    }

    func testDenySendsOpDeny() async {
        let env = FakeVoiceEnv(models: model(), confirms: [true])
        env.outcome = F.applied(verb: "deny")
        let lines = await VoiceFlow.run(.deny, env)
        XCTAssertEqual(env.sent.map(\.verb), [.opDeny(note: nil)])
        XCTAssertEqual(lines, ["Denied in payments-api."])
    }

    func testSettingsOffSendsNothingAndAsksNothing() async {
        for action in [VoiceAction.approveOnce, .approve, .deny] {
            let env = FakeVoiceEnv(models: model(), settings: F.settings(decisions: false, grants: true))
            let lines = await VoiceFlow.run(action, env)
            XCTAssertEqual(lines, [VoiceTexts.text("refusal.voiceOff")])
            XCTAssertTrue(env.sent.isEmpty)
            XCTAssertEqual(env.count("confirm"), 0)
        }
    }

    func testAnUnreadableOrDenyOnlyCommandIsNeverApproved() async {
        let floor = ApproveChoices(once: true, exact: true, limit: .floor)
        let denyOnly = FakeVoiceEnv(models: model(F.item(choices: floor)))
        let lines = await VoiceFlow.run(.approveOnce, denyOnly)
        XCTAssertEqual(lines, ["The command in payments-api can only be denied by voice: the approval floor holds it back."])
        XCTAssertTrue(denyOnly.sent.isEmpty)
        XCTAssertEqual(denyOnly.count("confirm"), 0)
        let denied = FakeVoiceEnv(models: model(F.item(choices: floor)))
        _ = await VoiceFlow.run(.deny, denied)
        XCTAssertEqual(denied.sent.count, 1)
        let unreadable = FakeVoiceEnv(models: model(F.item(command: "echo é")))
        let none = await VoiceFlow.run(.deny, unreadable)
        XCTAssertTrue(unreadable.sent.isEmpty)
        XCTAssertEqual(none.count, 1)
    }

    func testApproveWithAChoiceExactAndPrefix() async {
        let exact = FakeVoiceEnv(models: model(), confirms: [true], chooses: [1])
        _ = await VoiceFlow.run(.approve, exact)
        XCTAssertEqual(exact.sent.map(\.verb), [.opApproveAs(.exact)])
        XCTAssertEqual(exact.prompts[0].question, "How do you want to approve it? Say the number.")
        XCTAssertEqual(exact.prompts[0].options[0], "1: once.")
        XCTAssertEqual(exact.prompts[0].options.count, 5)
        XCTAssertEqual(exact.events.firstIndex(of: "choose")! < exact.events.firstIndex(of: "confirm")!, true)

        let prefix = FakeVoiceEnv(models: model(), confirms: [true], chooses: [2, 1])
        _ = await VoiceFlow.run(.approve, prefix)
        XCTAssertEqual(prefix.sent.map(\.verb), [.opApproveAs(.prefix(raw: "git push origin"))])
        XCTAssertEqual(prefix.prompts[1].options[1], "Choice 2 is: git push origin. End of choice 2.")
        XCTAssertTrue(prefix.confirmTexts[0].contains("And allow commands that start with choice 2: git push origin. End of choice 2."))
        XCTAssertTrue(prefix.confirmTexts[0].hasSuffix("Approve and allow?"))
    }

    func testNoNumberHeardSendsNothing() async {
        let env = FakeVoiceEnv(models: model(), chooses: [nil])
        let lines = await VoiceFlow.run(.approve, env)
        XCTAssertEqual(lines, ["I didn't catch a number. Nothing was sent."])
        XCTAssertTrue(env.sent.isEmpty)
        XCTAssertEqual(env.count("confirm"), 0)
        let outOfRange = FakeVoiceEnv(models: model(), chooses: [9])
        _ = await VoiceFlow.run(.approve, outOfRange)
        XCTAssertTrue(outOfRange.sent.isEmpty)
    }

    func testAWindowNeedsTheSecondYes() async {
        let one = FakeVoiceEnv(models: model(), confirms: [true, false], chooses: [3])
        let lines = await VoiceFlow.run(.approve, one)
        XCTAssertTrue(one.sent.isEmpty)
        XCTAssertEqual(one.count("confirm"), 2)
        XCTAssertEqual(lines, ["Nothing was sent."])
        XCTAssertTrue(one.confirmTexts[0].contains("The window approves every Operator command"))
        XCTAssertTrue(one.confirmTexts[0].hasSuffix("Approve and open the window?"))
        XCTAssertEqual(one.confirmTexts[1], "Last check: auto-approve everything in payments-api for 1 hour?")

        let first = FakeVoiceEnv(models: model(), confirms: [false, true], chooses: [3])
        _ = await VoiceFlow.run(.approve, first)
        XCTAssertTrue(first.sent.isEmpty)
        XCTAssertEqual(first.count("confirm"), 1)

        let both = FakeVoiceEnv(models: model(), confirms: [true, true], chooses: [3])
        both.outcome = F.applied(verb: "approve-as", approval: ApprovalResult(choice: "auto-hour", windowId: "win-7"))
        _ = await VoiceFlow.run(.approve, both)
        XCTAssertEqual(both.sent.map(\.verb), [.opApproveAs(.autoHour)])
        XCTAssertEqual(both.count("send"), 1)
    }

    func testGrantsOffOnlyOnceAndOldHostContinueAsApproveOnce() async {
        let grantsOff = FakeVoiceEnv(models: model(), settings: F.settings(grants: false))
        let lines = await VoiceFlow.run(.approve, grantsOff)
        XCTAssertEqual(lines.first, "Grants by voice is off, so only approve once is offered.")
        XCTAssertEqual(grantsOff.sent.map(\.verb), [.opApproveAs(.once)])
        XCTAssertEqual(grantsOff.count("choose"), 0)

        let old = FakeVoiceEnv(models: model(F.item(choices: nil)))
        let oldLines = await VoiceFlow.run(.approve, old)
        XCTAssertEqual(oldLines.first, "build-host offers only approve once. Update Capstan there for more.")
        XCTAssertEqual(old.sent.map(\.verb), [.opApprove])
        let oldOnce = FakeVoiceEnv(models: model(F.item(choices: nil)))
        _ = await VoiceFlow.run(.approveOnce, oldOnce)
        XCTAssertEqual(oldOnce.sent.map(\.verb), [.opApprove])

        let plain = ApproveChoices(once: true, exact: false)
        let single = FakeVoiceEnv(models: model(F.item(choices: plain)))
        let singleLines = await VoiceFlow.run(.approve, single)
        XCTAssertEqual(singleLines.first, "Only approve once is offered for this command.")
        XCTAssertEqual(single.sent.count, 1)
    }

    func testTheSendNeverRunsOutsideTheHostsRead() async {
        let env = FakeVoiceEnv(models: model())
        _ = await VoiceFlow.run(.approveOnce, env)
        XCTAssertFalse(env.sentOutside)
        let events = env.events
        XCTAssertTrue(events.firstIndex(of: "hosts.begin")! < events.firstIndex(of: "send")!)
        XCTAssertTrue(events.firstIndex(of: "send")! < events.firstIndex(of: "hosts.end")!)
    }

    func testTimeoutAndUnreachableAreSpokenAndSendNothing() async {
        let slow = FakeVoiceEnv(models: model())
        slow.hostError = .timeout(host: "build-host")
        let timeoutLines = await VoiceFlow.run(.approveOnce, slow)
        XCTAssertEqual(timeoutLines, ["build-host took too long. Nothing was sent, and Capstan won't retry."])
        XCTAssertTrue(slow.sent.isEmpty)
        let down = FakeVoiceEnv(models: model())
        down.hostError = .unreachable(host: "build-host")
        let downLines = await VoiceFlow.run(.deny, down)
        XCTAssertEqual(downLines, ["Could not reach build-host. Nothing was sent, and Capstan won't retry."])
    }

    func testNothingToDecideSendsNothing() async {
        let env = FakeVoiceEnv(models: [F.host(items: [])])
        let lines = await VoiceFlow.run(.approveOnce, env)
        XCTAssertEqual(lines, ["No command waits for you."])
        XCTAssertTrue(env.sent.isEmpty)
    }

    func testTheLeaseIsBegunAndEndedOncePerRun() async {
        let env = FakeVoiceEnv(models: model())
        env.background = true
        _ = await VoiceFlow.run(.approveOnce, env)
        XCTAssertEqual(env.count("lease.start"), 1)
        XCTAssertEqual(env.count("lease.stop"), 1)
        let failing = FakeVoiceEnv(models: model())
        failing.background = true
        let box = SessionLeaseBox()
        struct Boom: Error {}
        do {
            _ = try await box.hold(
                isBackground: { true }, start: { failing.note("start") }, stop: { failing.note("stop") },
                body: { () async throws -> Int in throw Boom() })
            XCTFail("expected a throw")
        } catch {
            XCTAssertEqual(failing.events, ["start", "stop"])
        }
    }

    // MARK: SessionLease

    func testSessionLeaseWithTwoHolders() {
        var lease = SessionLease()
        XCTAssertEqual(lease.begin(), .start)
        XCTAssertEqual(lease.begin(), .start)
        XCTAssertEqual(lease.end(isBackground: true), .none)
        XCTAssertEqual(lease.end(isBackground: true), .stop)
        XCTAssertEqual(lease.holders, 0)
    }

    func testSessionLeaseStopsOnlyInTheBackground() {
        var lease = SessionLease()
        _ = lease.begin()
        XCTAssertEqual(lease.end(isBackground: false), .none)
        _ = lease.begin()
        _ = lease.begin()
        XCTAssertEqual(lease.end(isBackground: false), .none)
        XCTAssertEqual(lease.end(isBackground: true), .stop)
        XCTAssertEqual(lease.holders, 0)
    }
}
