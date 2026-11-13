import CapstanKit
import Foundation

/// The words, glyphs and sentences of integration sets, from `docs/design/plan-185-ws-design-strings.json`. One
/// vocabulary for the inbox, the inspector, the merge sheet and the Workspace overview. The feed's `merged` reads
/// "built" here, never "merged": the host sent it for a member merged into the integration branch only.
public enum SetState: Sendable, Equatable {
    case building, built, inMain, conflicted, failed, discarded, dropped

    /// A member's word. A dropped member is `dropped` whatever its own state.
    public init(member: SetMember) {
        self = member.dropped ? .dropped : SetState(wire: member.state)
    }

    public init(wire: String) {
        switch wire {
        case "merged": self = .built
        case "confirmed": self = .inMain
        case "conflicted": self = .conflicted
        case "failed": self = .failed
        case "discarded": self = .discarded
        default: self = .building
        }
    }

    public var word: String {
        switch self {
        case .building: return "building"
        case .built: return "built"
        case .inMain: return "in main"
        case .conflicted: return "conflicted"
        case .failed: return "failed"
        case .discarded: return "discarded"
        case .dropped: return "dropped"
        }
    }

    public var longWord: String {
        switch self {
        case .built: return "Built, not in main"
        default: return word.prefix(1).uppercased() + word.dropFirst()
        }
    }

    public var voiceover: String {
        switch self {
        case .built: return "built, not in main"
        case .dropped: return "dropped, outside the gate, review and confirm"
        default: return word
        }
    }

    public var glyph: String {
        switch self {
        case .building: return "◌"
        case .built: return "○"
        case .inMain: return "●"
        case .conflicted: return "▲"
        case .failed: return "✕"
        case .discarded, .dropped: return "–"
        }
    }

    /// The SF Symbol of the state.
    public var symbol: String {
        switch self {
        case .building: return "circle.dotted"
        case .built: return "circle"
        case .inMain: return "circle.fill"
        case .conflicted: return "exclamationmark.triangle.fill"
        case .failed: return "xmark.circle"
        case .discarded, .dropped: return "minus.circle"
        }
    }

    public var tone: Tone {
        switch self {
        case .building, .discarded, .dropped: return .grey
        case .built: return .blue
        case .inMain: return .green
        case .conflicted: return .yellow
        case .failed: return .red
        }
    }
}

public enum SetText {
    /// Glyphs the strip shows before `+n` (the design's cap on the Mac).
    public static let stripCap = 12

    // MARK: Set

    /// The key of the waiting item of another repo of the same set, when the host holds one. "Open <repo>" and
    /// "Merge <next>…" are offered only for a repo that has an item to show; the Mac and the phone share this.
    public static func waitingKey(of repo: String, in set: IntegrationSet, among waiting: [StoredWaiting]) -> String? {
        waiting.first { $0.set?.repo == repo && $0.set?.setId == set.setId }?.key
    }

    /// The set's word and sentence (`set.summary.*`).
    public static func state(_ set: IntegrationSet) -> SetState {
        let live = set.liveMembers
        if set.state == "discarded" { return .discarded }
        if live.contains(where: { $0.state == "conflicted" }) || set.state == "conflicted" { return .conflicted }
        if live.contains(where: { $0.state == "failed" }) || set.state == "failed" { return .failed }
        if set.isInMain { return .inMain }
        if live.contains(where: { SetState(member: $0) == .building }) { return .building }
        return .built
    }

    public static func summary(_ set: IntegrationSet) -> String {
        let live = set.liveMembers.count
        let inMain = set.inMainCount
        switch state(set) {
        case .discarded:
            return inMain == 0 ? "discarded · nothing reached main" : "discarded · \(inMain) of \(live) stay in main"
        case .conflicted: return "conflicted in " + (firstRepo(set, "conflicted") ?? set.repo)
        case .failed: return "failed in " + (firstRepo(set, "failed") ?? set.repo)
        case .inMain: return "in main · all \(live) repos"
        case .building: return "building · \(set.builtCount) of \(live) built"
        default: return inMain == 0 ? "built · none in main yet" : "\(inMain) of \(live) in main"
        }
    }

    /// The part of `set.title.app` after the id: building, built, conflicted in x, failed in x, 1 of 3 in main, in main
    /// or discarded.
    public static func summaryTitle(_ set: IntegrationSet) -> String {
        let live = set.liveMembers.count
        switch state(set) {
        case .building: return "building"
        case .conflicted, .failed: return summary(set)
        case .inMain: return "in main"
        case .discarded: return "discarded"
        default: return set.inMainCount == 0 ? "built" : "\(set.inMainCount) of \(live) in main"
        }
    }

    /// `set.title.app`: "Set 8a46cad2 · 1 of 3 in main".
    public static func title(_ set: IntegrationSet) -> String {
        "Set " + set.setId + " · " + summaryTitle(set)
    }

    /// Why the set waits (the second line of its row), nil when nothing waits on a person.
    public static func reason(_ set: IntegrationSet) -> String? {
        let attention = set.liveMembers.first { $0.state == "conflicted" } ?? set.liveMembers.first { $0.state == "failed" }
        if let member = attention {
            if member.state == "conflicted" { return member.repo + " conflicted: redo it or message the PM" }
            return member.repo + " failed" + (member.isRedone ? ", attempt \(member.attempt)" : "")
        }
        if let next = set.next, state(set) != .inMain, state(set) != .discarded {
            return state(set) == .building ? nil : "Merge " + next.repo + " next"
        }
        return nil
    }

    private static func firstRepo(_ set: IntegrationSet, _ wire: String) -> String? {
        set.liveMembers.first { $0.state == wire }?.repo
    }

    // MARK: Strip

    /// The glyphs of the strip: the first `cap` in merge order, then `+n`. When the next repo is beyond them the strip
    /// starts two before it, with a leading `+n` for those left out.
    public struct Strip: Sendable, Equatable {
        public let leading: Int
        public let members: [SetMember]
        public let trailing: Int

        public var glyphs: String { members.map { SetState(member: $0).glyph }.joined() }

        public var text: String {
            (leading > 0 ? "+\(leading) " : "") + glyphs + (trailing > 0 ? " +\(trailing)" : "")
        }
    }

    public static func strip(_ set: IntegrationSet, cap: Int = stripCap) -> Strip {
        let all = set.members
        guard all.count > cap else { return Strip(leading: 0, members: all, trailing: 0) }
        var start = 0
        if let next = set.next, let index = all.firstIndex(where: { $0.repo == next.repo }), index >= cap {
            start = min(max(0, index - 2), all.count - cap)
        }
        let shown = Array(all[start..<(start + cap)])
        return Strip(leading: start, members: shown, trailing: all.count - start - cap)
    }

    // MARK: VoiceOver

    /// One element: the summary, then each member's repo and state in merge order (the first `cap`, then "and n more
    /// repos").
    public static func stripVoiceover(_ set: IntegrationSet, cap: Int = stripCap) -> String {
        let shown = set.members.prefix(cap).map { $0.repo + " " + SetState(member: $0).voiceover }
        var text = summary(set) + ". " + shown.joined(separator: ", ")
        let more = set.members.count - shown.count
        if more > 0 { text += ", and \(more) more repos" }
        return text
    }

    /// `a11y.set.row`: "Set 8a46cad2, 1 of 3 in main, next web".
    public static func rowVoiceover(_ set: IntegrationSet) -> String {
        var text = "Set " + set.setId + ", " + summaryTitle(set)
        if let next = set.next, state(set) != .inMain, state(set) != .discarded { text += ", next " + next.repo }
        return text
    }
}

/// One repo of a set as the inspector and the sheets list it.
public struct SetMemberRow: Sendable, Equatable, Identifiable {
    public var id: String { repo }
    /// 1-based merge order.
    public let position: Int
    public let total: Int
    public let repo: String
    public let state: SetState
    /// `<headSha> · verify <word>`, `no head yet` or the dropped note.
    public let sub: String
    public let isNext: Bool
    public let isReviewSubject: Bool
    /// `attempt n`, only after a redo.
    public let attempt: String?
    public let struck: Bool
    public let voiceover: String
    /// The member this row is about (the item that is open).
    public let isCurrent: Bool

    public static let nextTag = "Next"
    public static let reviewSubjectTag = "Review subject"
    public static let droppedNote = "Outside the gate, review and confirm"

    /// The rows of a set in merge order. `verify` is the item's `verify.byRepo` when the host sent it.
    public static func rows(_ set: IntegrationSet, verify: [RepoVerify]? = nil) -> [SetMemberRow] {
        let next = set.next?.repo
        let subject = set.reviewSubject ? set.repo : nil
        return set.members.enumerated().map { index, member in
            let state = SetState(member: member)
            let isNext = member.repo == next && state != .dropped
            let isSubject = member.repo == subject
            let attempt = member.isRedone ? "attempt \(member.attempt)" : nil
            var voice = "\(index + 1) of \(set.members.count), \(member.repo), \(state.voiceover)"
            if isSubject { voice += ", review subject" }
            if isNext { voice += ", next to merge" }
            if let attempt { voice += ", " + attempt }
            return SetMemberRow(
                position: index + 1, total: set.members.count, repo: member.repo, state: state,
                sub: sub(member, state: state, verify: verify?.first { $0.repo == member.repo }), isNext: isNext,
                isReviewSubject: isSubject, attempt: attempt, struck: member.dropped, voiceover: voice,
                isCurrent: member.repo == set.repo)
        }
    }

    static func sub(_ member: SetMember, state: SetState, verify: RepoVerify?) -> String {
        if state == .dropped { return droppedNote }
        guard let head = member.headSha else { return "no head yet" }
        return String(head.prefix(7)) + " · verify " + verifyWord(verify)
    }

    /// The shipped run state, or "not run" while the repo has no finished run.
    public static func verifyWord(_ verify: RepoVerify?) -> String {
        guard let state = verify?.latest?.state else { return "not run" }
        // A queued run is waiting for a slot of the same run: the apps call it running.
        return state == "queued" ? "running" : state
    }
}

/// The gate of a set, read from `verify.byRepo` the way the host reads it: failed names the live repos whose latest
/// run failed, else waiting names those running or without a run, else passed.
public struct SetGate: Sendable, Equatable {
    public enum State: Sendable, Equatable { case passed, waiting, failed }

    public let state: State
    public let repos: [String]
    public let passed: Int
    public let running: Int

    public var line: String {
        switch state {
        case .passed: return "gate passed: every repo's latest run passed"
        case .waiting: return "gate waiting: " + repos.joined(separator: ", ")
        case .failed: return "gate failed: " + repos.joined(separator: ", ")
        }
    }

    /// `verify.gate.leadWaiting`.
    public var lead: String? {
        state == .waiting ? "Every repo’s latest run must pass. \(passed) passed; \(running) is running." : nil
    }

    public static func make(_ set: IntegrationSet, verify: [RepoVerify]?) -> SetGate? {
        let live = set.liveMembers
        guard !live.isEmpty, let verify else { return nil }
        func run(_ repo: String) -> String? { verify.first { $0.repo == repo }?.latest?.state }
        let failed = live.filter { ["failed", "timeout", "error"].contains(run($0.repo) ?? "") }.map(\.repo)
        let open = live.filter { run($0.repo) == nil || run($0.repo) == "running" || run($0.repo) == "queued" }.map(\.repo)
        let passed = live.filter { run($0.repo) == "passed" }.count
        let running = live.filter { run($0.repo) == "running" || run($0.repo) == "queued" }.count
        if !failed.isEmpty { return SetGate(state: .failed, repos: failed, passed: passed, running: running) }
        if !open.isEmpty { return SetGate(state: .waiting, repos: open, passed: passed, running: running) }
        return SetGate(state: .passed, repos: [], passed: passed, running: running)
    }

    /// The per-repo verify line of a member: `web failed · last passed 51ab07e`, `worker not run`, or `infra outside
    /// the gate` for a dropped member.
    public static func repoLine(_ row: RepoVerify?, member: SetMember) -> String {
        if member.dropped { return member.repo + " outside the gate" }
        var text = member.repo + " " + SetMemberRow.verifyWord(row)
        if let latest = row?.latest, latest.state != "passed", let last = row?.lastPassed {
            text += " · last passed " + String(last.commitSha.prefix(7))
        }
        return text
    }
}
