import CapstanKit
import XCTest

@testable import CapstanAppCore

final class SystemTests: XCTestCase {
    private let host = UUID(uuidString: "11111111-1111-1111-1111-111111111111")!

    private func candidate(_ kind: WaitingKind, key: String = "op:7") -> NotifyCandidate {
        NotifyCandidate(hostId: host, projectId: "proj", kind: kind, key: key)
    }

    private func plan(
        _ kind: WaitingKind = .op, settings: AppSettings = AppSettings(), project: String = "Alpha",
        prefs: SoundPreferences = SoundPreferences(), check: @escaping (String) -> Bool = { _ in true }
    ) -> NotificationPlan {
        NotificationPlan(
            candidate: candidate(kind), settings: settings, projectName: project, preferences: prefs, fileCheck: check)
    }

    private func row(_ key: String, since: String, forwarded: Bool = false) -> InboxRow {
        InboxRow(
            id: InboxRowID(hostId: host, projectId: "proj", key: key), kind: .op, projectName: "Alpha", since: since,
            summary: UntrustedText(raw: "Run tests", verified: true, truncated: false), needsTerminal: false,
            forwarded: forwarded, stale: false, requestId: key, contentHash: "h")
    }

    private final class MemoryStore: SoundStore {
        var values: [String: Data] = [:]
        func data(forKey key: String) -> Data? { values[key] }
        func set(_ data: Data?, forKey key: String) { values[key] = data }
    }

    // MARK: NotificationPlan

    func test_titles() {
        XCTAssertEqual(plan(.op).title, "Command proposal waiting")
        XCTAssertEqual(plan(.prompt).title, "Permission prompt waiting")
        XCTAssertEqual(plan(.design).title, "Design approval waiting")
        XCTAssertEqual(plan(.message).title, "Message waiting")
        XCTAssertEqual(plan(.integration).title, "Ready to merge")
    }

    func test_subtitleIsCleanedAndCut() {
        XCTAssertEqual(plan(project: "Alpha").subtitle, "Alpha")
        XCTAssertEqual(plan(project: "A\nB\u{202E}C").subtitle, "A B C")
        XCTAssertEqual(plan(project: String(repeating: "x", count: 100)).subtitle.count, 64)
    }

    func test_interruptionFollowsFocusSetting() {
        XCTAssertEqual(plan(settings: AppSettings(allowDuringFocus: false)).interruption, .active)
        XCTAssertEqual(plan(settings: AppSettings(allowDuringFocus: true)).interruption, .timeSensitive)
    }

    func test_userInfoHasExactlyTheThreeKeysAndRoundTrips() {
        let p = plan()
        XCTAssertEqual(Set(p.userInfo.keys), ["hostId", "projectId", "key"])
        XCTAssertEqual(
            NotificationPlan.row(fromUserInfo: p.userInfo),
            InboxRowID(hostId: host, projectId: "proj", key: "op:7"))
    }

    func test_rowFromUserInfoNilWhenAKeyIsMissing() {
        for missing in ["hostId", "projectId", "key"] {
            var info = plan().userInfo
            info[missing] = nil
            XCTAssertNil(NotificationPlan.row(fromUserInfo: info), missing)
        }
        var bad = plan().userInfo
        bad["hostId"] = "not-an-id"
        XCTAssertNil(NotificationPlan.row(fromUserInfo: bad))
    }

    // MARK: SoundCatalog

    func test_catalogOrderAndFiltering() {
        let catalog = SoundCatalog(
            systemFiles: ["Submarine.aiff", "Basso.aiff", ".hidden.aiff", "notes.txt"],
            userFiles: ["zed.wav", "Alpha.CAF", "clip.mp3", "b.aiff", "dir/x.wav"])
        XCTAssertEqual(catalog.entries.map(\.title), ["None", "Basso", "Submarine", "Alpha", "b", "zed"])
        XCTAssertEqual(catalog.entries.first?.choice, .silent)
        XCTAssertEqual(catalog.system.map(\.choice), [.system("Basso.aiff"), .system("Submarine.aiff")])
        XCTAssertEqual(catalog.user.map(\.choice), [.user("Alpha.CAF"), .user("b.aiff"), .user("zed.wav")])
    }

    func test_catalogWithNothingHasOnlyNone() {
        XCTAssertEqual(SoundCatalog(systemFiles: [], userFiles: []).entries.map(\.title), ["None"])
    }

    // MARK: SoundPreferences

    func test_resolveFallsBackForMissingUserSound() {
        let prefs = SoundPreferences(defaultSound: .system("Basso.aiff"), perKind: ["op": .user("mine.wav")])
        XCTAssertEqual(prefs.resolve(.op, fileCheck: { _ in true }), .user("mine.wav"))
        XCTAssertEqual(prefs.resolve(.op, fileCheck: { _ in false }), .system("Basso.aiff"))
        XCTAssertEqual(prefs.resolve(.message, fileCheck: { _ in false }), .system("Basso.aiff"))
    }

    func test_resolveFallsBackToBuiltInWhenDefaultIsGoneToo() {
        let prefs = SoundPreferences(defaultSound: .user("gone.wav"))
        XCTAssertEqual(prefs.resolve(.op, fileCheck: { _ in false }), SoundPreferences.builtIn)
    }

    func test_resolveGivesNilForNone() {
        let prefs = SoundPreferences(defaultSound: .system("Basso.aiff"), perKind: ["op": .silent])
        XCTAssertNil(prefs.resolve(.op, fileCheck: { _ in true }))
        XCTAssertNil(SoundPreferences(defaultSound: .silent).resolve(.message, fileCheck: { _ in true }))
        XCTAssertNil(plan(.op, prefs: prefs).sound)
    }

    func test_planCarriesTheResolvedSound() {
        let prefs = SoundPreferences(perKind: ["prompt": .user("mine.wav")])
        XCTAssertEqual(plan(.prompt, prefs: prefs).sound, .user("mine.wav"))
        XCTAssertEqual(plan(.prompt, prefs: prefs, check: { _ in false }).sound, SoundPreferences.builtIn)
    }

    func test_preferencesSurviveAStoreRoundTrip() {
        let store = MemoryStore()
        XCTAssertEqual(SoundPreferences.load(from: store), SoundPreferences())
        let prefs = SoundPreferences(
            defaultSound: .user("mine.wav"), perKind: ["op": .silent, "design": .system("Glass.aiff")])
        prefs.save(to: store)
        XCTAssertNotNil(store.values["sounds.v1"])
        XCTAssertEqual(SoundPreferences.load(from: store), prefs)
        store.values["sounds.v1"] = Data("garbage".utf8)
        XCTAssertEqual(SoundPreferences.load(from: store), SoundPreferences())
    }

    // MARK: PreviewState

    func test_previewState() {
        XCTAssertEqual(PreviewState.make(.silent, fileCheck: { _ in true }), .disabled)
        XCTAssertEqual(PreviewState.make(.user("gone.wav"), fileCheck: { _ in false }), .unavailable)
        XCTAssertEqual(PreviewState.make(.user("here.wav"), fileCheck: { _ in true }), .playable)
        XCTAssertEqual(PreviewState.make(.system("Glass.aiff"), fileCheck: { _ in true }), .playable)
    }

    // MARK: AuthorizationPolicy

    func test_authorizationAtLaunch() {
        let on: Set<WaitingKind> = [.op]
        XCTAssertTrue(AuthorizationPolicy.shouldRequest(status: .notDetermined, kindsOn: on, previousKinds: nil))
        XCTAssertFalse(AuthorizationPolicy.shouldRequest(status: .notDetermined, kindsOn: [], previousKinds: nil))
        for status in [AuthStatus.denied, .authorized, .provisional, .ephemeral] {
            XCTAssertFalse(AuthorizationPolicy.shouldRequest(status: status, kindsOn: on, previousKinds: nil))
        }
    }

    func test_authorizationWhenAKindIsTurnedOn() {
        XCTAssertTrue(
            AuthorizationPolicy.shouldRequest(status: .notDetermined, kindsOn: [.op, .prompt], previousKinds: [.op]))
        XCTAssertFalse(
            AuthorizationPolicy.shouldRequest(status: .notDetermined, kindsOn: [.op], previousKinds: [.op, .prompt]))
        XCTAssertFalse(
            AuthorizationPolicy.shouldRequest(status: .denied, kindsOn: [.op, .prompt], previousKinds: [.op]))
    }

    // MARK: MenuList

    func test_menuListOfSevenShowsFiveAndTwoMore() {
        let rows = (1...7).map { row("op:\($0)", since: "2026-10-01T10:0\($0):00Z") }
        let list = MenuList(rows: rows)
        XCTAssertEqual(list.shown.count, 5)
        XCTAssertEqual(list.moreCount, 2)
        XCTAssertEqual(list.shown.first?.id.key, "op:7")
        XCTAssertEqual(list.shown.last?.id.key, "op:3")
    }

    func test_menuListSkipsForwardedAndHandlesFew() {
        let list = MenuList(rows: [row("a", since: "2026-10-01T10:00:00Z"), row("b", since: "2026-10-01T11:00:00Z", forwarded: true)])
        XCTAssertEqual(list.shown.map(\.id.key), ["a"])
        XCTAssertEqual(list.moreCount, 0)
        XCTAssertEqual(MenuList(rows: []).shown.count, 0)
    }

    func test_menuAge() {
        let now = MenuList.date("2026-10-01T12:00:00Z")!
        XCTAssertEqual(MenuList.age(since: "2026-10-01T11:59:50Z", now: now), "now")
        XCTAssertEqual(MenuList.age(since: "2026-10-01T11:55:00Z", now: now), "5m")
        XCTAssertEqual(MenuList.age(since: "2026-10-01T09:00:00Z", now: now), "3h")
        XCTAssertEqual(MenuList.age(since: "2026-09-29T12:00:00Z", now: now), "2d")
        XCTAssertEqual(MenuList.age(since: "garbage", now: now), "")
    }

    func test_menuSummaryIsPlainAndUnverifiedIsHidden() {
        let multi = row("a", since: "2026-10-01T10:00:00Z")
        XCTAssertEqual(MenuList.summaryLine(multi), "Run tests")
        let unverified = InboxRow(
            id: multi.id, kind: .op, projectName: "Alpha", since: multi.since,
            summary: UntrustedText(raw: "secret", verified: false, truncated: false), needsTerminal: false,
            forwarded: false, stale: false, requestId: "r", contentHash: "h")
        XCTAssertEqual(MenuList.summaryLine(unverified), "Text not shown")
    }
}
