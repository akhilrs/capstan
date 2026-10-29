import CapstanKit
import XCTest

@testable import CapstanAppCore

final class HostsTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func text(_ s: String, verified: Bool = true) -> UntrustedText {
        UntrustedText(raw: s, verified: verified, truncated: false)
    }

    private func banners(
        _ state: ConnectionState?, loading: Bool = false, decide: Bool? = nil, name: String = "box",
        destination: String = "me@box"
    ) -> [BannerSpec] {
        BannerChoice.make(
            state: state, anyLoading: loading, decide: decide, hostName: name, destination: destination,
            now: now)
    }

    private func reconnecting(_ error: SessionError?) -> ConnectionState {
        .reconnecting(attempt: 2, retryAt: now.addingTimeInterval(5), lastError: error)
    }

    private var live: ConnectionState { .live(since: now, lastLineAt: now) }

    // MARK: ProjectName, AvailabilityText, anyLoading

    func test_ProjectName_paths() {
        XCTAssertEqual(ProjectName.display(root: "/a/b"), "b")
        XCTAssertEqual(ProjectName.display(root: "/a/b/"), "b")
        XCTAssertEqual(ProjectName.display(root: "/"), "/")
    }

    func test_AvailabilityText_allStates() {
        let states: [ProjectAvailability] = [.ready, .noKey, .silent, .refused, .unsupported, .gone]
        var words = Set<String>()
        for state in states {
            let t = AvailabilityText(state)
            XCTAssertFalse(t.word.isEmpty)
            XCTAssertFalse(t.symbol.isEmpty)
            XCTAssertFalse(t.meaning.isEmpty)
            XCTAssertFalse(t.fix.isEmpty)
            words.insert(t.word)
        }
        XCTAssertEqual(words.count, 6)
    }

    func test_AvailabilityText_other() {
        let t = AvailabilityText(.other("x\u{202E}y\nz"))
        XCTAssertEqual(t.word, "Unknown")
        XCTAssertFalse(t.meaning.contains("\u{202E}"))
        XCTAssertFalse(t.meaning.contains("\n"))
    }

    func test_anyLoading() {
        let done = ProjectModel(projectId: "a", root: "/a", snapshotComplete: true)
        let open = ProjectModel(projectId: "b", root: "/b", snapshotComplete: false)
        let id = UUID()
        XCTAssertFalse(anyLoading(HostModel(hostId: id)))
        XCTAssertFalse(anyLoading(HostModel(hostId: id, projects: ["a": done])))
        XCTAssertTrue(anyLoading(HostModel(hostId: id, projects: ["a": done, "b": open])))
        XCTAssertFalse(anyLoading(nil))
    }

    // MARK: CapsuleChoice

    private func capsule(_ state: ConnectionState?, loading: Bool = false, pulse: Bool = true, reduce: Bool = false)
        -> CapsuleSpec
    {
        CapsuleChoice.make(state: state, anyLoading: loading, heartbeatPulse: pulse, reduceMotion: reduce)
    }

    func test_Capsule_nilAndConnecting() {
        XCTAssertEqual(capsule(nil).label, "Not connected")
        XCTAssertEqual(capsule(.connecting).label, "Connecting")
        XCTAssertFalse(capsule(.connecting).pulse)
    }

    func test_Capsule_live() {
        XCTAssertEqual(capsule(live).label, "Live")
        XCTAssertEqual(capsule(live).tone, .green)
        XCTAssertEqual(capsule(live, loading: true).label, "Loading")
        XCTAssertEqual(capsule(live, loading: true).tone, .blue)
    }

    func test_Capsule_pulseRule() {
        XCTAssertTrue(capsule(live).pulse)
        XCTAssertFalse(capsule(live, pulse: false).pulse)
        XCTAssertFalse(capsule(live, reduce: true).pulse)
        XCTAssertFalse(capsule(live, loading: true).pulse)
        XCTAssertFalse(capsule(.noHeartbeat(since: now)).pulse)
    }

    func test_Capsule_noHeartbeatAndReconnecting() {
        XCTAssertEqual(capsule(.noHeartbeat(since: now)).label, "No heartbeat")
        XCTAssertEqual(capsule(reconnecting(nil)).label, "Reconnecting")
        XCTAssertEqual(capsule(reconnecting(nil), loading: true).label, "Reconnecting")
    }

    func test_Capsule_eachStopReason() {
        let reasons: [StopReason] = [
            .user, .unsupportedVersion(2), .hostRejected(code: "x", tail: text("")),
            .cstanMissing(tail: text("")), .repeatedFailure,
        ]
        for reason in reasons {
            let spec = capsule(.stopped(reason), loading: true)
            XCTAssertEqual(spec.label, "Stopped")
            XCTAssertFalse(spec.pulse)
        }
    }

    // MARK: BannerChoice, states

    func test_Banner_noneForQuietStates() {
        XCTAssertTrue(banners(nil).isEmpty)
        XCTAssertTrue(banners(.connecting).isEmpty)
        XCTAssertTrue(banners(live).isEmpty)
        XCTAssertTrue(banners(.stopped(.user)).isEmpty)
    }

    func test_Banner_noHeartbeat() {
        let b = banners(.noHeartbeat(since: now.addingTimeInterval(-45.9)))
        XCTAssertEqual(b.count, 1)
        XCTAssertEqual(b[0].text, "No heartbeat for 45 s")
        XCTAssertEqual(b[0].tone, .yellow)
        XCTAssertEqual(b[0].kind, .state)
    }

    func test_Banner_reconnectingWithoutMessage() {
        XCTAssertTrue(banners(reconnecting(nil)).isEmpty)
        XCTAssertTrue(banners(reconnecting(.interrupted)).isEmpty)
        XCTAssertTrue(banners(reconnecting(.exit(0, tail: text("")))).isEmpty)
    }

    func test_Banner_ssh() {
        let b = banners(reconnecting(.ssh(tail: text("connection refused"))), name: "box")
        XCTAssertEqual(b.count, 1)
        XCTAssertEqual(b[0].text, "Could not reach box over SSH")
        XCTAssertEqual(b[0].detail?.raw, "connection refused")
        XCTAssertEqual(b[0].retryAt, now.addingTimeInterval(5))
        XCTAssertNil(b[0].action)
    }

    func test_Banner_sshHints() {
        for tail in ["Host key verification failed.", "me@box: Permission denied (publickey)."] {
            let b = banners(reconnecting(.ssh(tail: text(tail))), destination: "me@box")
            XCTAssertTrue(b[0].text.hasSuffix("Run ssh me@box once in a terminal."), b[0].text)
        }
        let plain = banners(reconnecting(.ssh(tail: text("timed out"))))
        XCTAssertFalse(plain[0].text.contains("Run ssh"))
    }

    func test_Banner_sshTailCap() {
        let exact = String(repeating: "a", count: 4096)
        let b = banners(reconnecting(.ssh(tail: text(exact))))
        XCTAssertEqual(b[0].detail?.raw.utf8.count, 4096)
        XCTAssertEqual(b[0].detail?.truncated, false)
        let over = banners(reconnecting(.ssh(tail: text(exact + "a"))))
        XCTAssertEqual(over[0].detail?.raw.utf8.count, 4096)
        XCTAssertEqual(over[0].detail?.truncated, true)
    }

    func test_Banner_tailCutBeforeWideCharacter() {
        let tail = String(repeating: "a", count: 4094) + "\u{1F600}"
        let b = banners(reconnecting(.ssh(tail: text(tail))))
        XCTAssertEqual(b[0].detail?.raw, String(repeating: "a", count: 4094))
        XCTAssertEqual(b[0].detail?.truncated, true)
    }

    func test_Banner_unverifiedTailHasNoDetail() {
        let b = banners(reconnecting(.ssh(tail: text("secret", verified: false))))
        XCTAssertNil(b[0].detail)
    }

    func test_Banner_streamError() {
        let b = banners(reconnecting(.stream(code: "bad_cursor", message: text("m"))), name: "box")
        XCTAssertEqual(b[0].text, "Capstan on box stopped the stream: bad_cursor")
    }

    func test_Banner_exitCodes() {
        func message(_ n: Int32) -> String? {
            banners(reconnecting(.exit(n, tail: text("")))).first?.text
        }
        XCTAssertNil(message(0))
        XCTAssertEqual(message(-1), "The stream ended with exit -1")
        XCTAssertEqual(message(1), "The stream ended with an error (exit 1)")
        XCTAssertEqual(message(3), "The stream ended with exit 3")
        XCTAssertEqual(message(128), "The stream ended with exit 128")
        XCTAssertEqual(message(137), "The stream ended with exit 137 (signal 9)")
    }

    func test_Banner_stops() {
        let unsupported = banners(.stopped(.unsupportedVersion(9)))[0]
        XCTAssertEqual(unsupported.text, "Update this app")
        XCTAssertNil(unsupported.detail)
        XCTAssertNil(unsupported.action)

        let rejected = banners(.stopped(.hostRejected(code: "denied", tail: text("why"))))[0]
        XCTAssertEqual(rejected.text, "The host refused: denied")
        XCTAssertEqual(rejected.detail?.raw, "why")
        XCTAssertNil(rejected.action)

        let missing = banners(.stopped(.cstanMissing(tail: text("sh: cstan: not found"))), name: "box")[0]
        XCTAssertEqual(missing.text, "cstan was not found on box; set the cstan path")
        XCTAssertEqual(missing.detail?.raw, "sh: cstan: not found")
        XCTAssertEqual(missing.action, .editHost)

        let repeated = banners(.stopped(.repeatedFailure))[0]
        XCTAssertEqual(repeated.text, "The stream failed 5 times")
        XCTAssertEqual(repeated.action, .retry)
    }

    // MARK: BannerChoice, extra banners

    func test_Banner_noDecide() {
        let b = banners(live, decide: false)
        XCTAssertEqual(b.map(\.kind), [.noDecide])
        XCTAssertEqual(b[0].tone, .grey)
        XCTAssertTrue(banners(live, decide: nil).isEmpty)
        XCTAssertTrue(banners(live, decide: true).isEmpty)
    }

    func test_Banner_failureThenNoDecide() {
        let b = banners(.stopped(.repeatedFailure), decide: false)
        XCTAssertEqual(b.map(\.kind), [.state, .noDecide])
        let c = banners(reconnecting(.ssh(tail: text("x"))), decide: false)
        XCTAssertEqual(c.map(\.kind), [.state, .noDecide])
    }

    func test_Banner_loading() {
        let b = banners(live, loading: true)
        XCTAssertEqual(b.map(\.kind), [.loading])
        XCTAssertEqual(b[0].tone, .blue)
        XCTAssertEqual(b[0].text, "Loading projects")
        XCTAssertEqual(banners(live, loading: true, decide: false).map(\.kind), [.loading, .noDecide])
        XCTAssertTrue(banners(.stopped(.user), loading: true).isEmpty)
        XCTAssertTrue(banners(reconnecting(nil), loading: true).isEmpty)
        XCTAssertTrue(banners(.connecting, loading: true).isEmpty)
    }

    func test_Banner_hostileStrings() {
        let hostile = "\u{202E}evil\nname" + String(repeating: "x", count: 10_240)
        let b = banners(
            reconnecting(.stream(code: hostile, message: text(""))), name: hostile, destination: hostile)
        let shown = b[0].text
        XCTAssertFalse(shown.contains("\u{202E}"))
        XCTAssertFalse(shown.contains("\n"))
        XCTAssertLessThanOrEqual(shown.utf8.count, 64 + 256 + 64)
        XCTAssertTrue(shown.contains(" evil name"))

        let ssh = banners(
            reconnecting(.ssh(tail: text("Permission denied"))), name: hostile, destination: hostile)[0]
        XCTAssertFalse(ssh.text.contains("\u{202E}"))
        XCTAssertLessThanOrEqual(ssh.text.utf8.count, 400)
    }

    // MARK: countdown, actions

    func test_countdown() {
        XCTAssertEqual(countdown(retryAt: now.addingTimeInterval(11.2), now: now), "Retrying in 12 s")
        XCTAssertEqual(countdown(retryAt: now, now: now), "Retrying now")
        XCTAssertEqual(countdown(retryAt: now.addingTimeInterval(-3), now: now), "Retrying now")
    }

    func test_resolveAction() {
        XCTAssertEqual(BannerChoice.resolveAction(.retry, configExists: false, sheetPresented: true), .restart)
        XCTAssertEqual(
            BannerChoice.resolveAction(.editHost, configExists: true, sheetPresented: false), .openEditor)
        XCTAssertEqual(
            BannerChoice.resolveAction(.editHost, configExists: false, sheetPresented: false), .none)
        XCTAssertEqual(
            BannerChoice.resolveAction(.editHost, configExists: true, sheetPresented: true), .none)
    }

    // MARK: CheckStepText

    func test_CheckStepText_stepsAndStatuses() {
        let steps: [HostCheckStep] = [.connect, .capstan, .projects, .firstSnapshot]
        let statuses: [HostCheckStatus] = [.passed, .warning, .failed]
        var titles = Set<String>()
        for step in steps {
            for status in statuses {
                let t = CheckStepText.make(HostCheck(step: step, status: status, text: "hello"), destination: "d")
                XCTAssertFalse(t.title.isEmpty)
                XCTAssertEqual(t.line, "hello")
                titles.insert(t.title)
            }
        }
        XCTAssertEqual(titles.count, 4)
    }

    func test_CheckStepText_emptyTextUsesStatus() {
        let t = CheckStepText.make(HostCheck(step: .projects, status: .warning, text: ""), destination: "d")
        XCTAssertEqual(t.line, "Warning")
    }

    func test_CheckStepText_hintsAndCleaning() {
        for tail in ["Host key verification failed.", "Permission denied (publickey)."] {
            let t = CheckStepText.make(
                HostCheck(step: .connect, status: .failed, text: tail), destination: "me@box")
            XCTAssertTrue(t.line.hasSuffix("Run ssh me@box once in a terminal."), t.line)
        }
        let t = CheckStepText.make(
            HostCheck(step: .connect, status: .failed, text: "a\u{202E}b\nc"), destination: "d")
        XCTAssertEqual(t.line, "a b c")
    }

    // MARK: pollError

    func test_pollError() {
        func error(_ ms: Int) -> String? {
            HostLimits.pollError(HostConfig(displayName: "h", destination: "box", pollMs: ms))
        }
        XCTAssertEqual(HostLimits.maxPollMs, 3_600_000)
        XCTAssertEqual(error(999), HostConfigError.pollMs.message)
        XCTAssertNil(error(1000))
        XCTAssertNil(error(3_600_000))
        XCTAssertEqual(error(3_600_001), "At most 3600000 ms")
    }
}
