import CapstanKit
import Foundation

/// The merge sheet of a member of an integration set: one repo is decided at a time, in merge order. The sentences are
/// the design's (`docs/design/plan-185-ws-design-strings.json`, `decide.*` and `confirm.*`); the host's refusal
/// "merge <x> first" is quoted exactly as sent.
public struct SetMergeSheet: Sendable, Equatable {
    /// `Merge web into main`.
    public let title: String
    /// `Repo 2 of 3 in merge order`.
    public let order: String
    public let members: [SetMemberRow]
    public let gate: SetGate?
    /// `After this: worker`, or `This is the last repo of the set.`
    public let after: String
    /// `Merge web`.
    public let action: String
    /// The member that has to be merged first, when this one is not next; the sheet then shows the refusal at once
    /// and offers `Open <repo>` instead of Merge.
    public let blockedBy: String?
    /// `worker has to wait`.
    public let blockedTitle: String?

    /// Why the blocking member cannot be merged yet, when it is still running, conflicted or failed: it is still the
    /// one that is next, so this repo waits for it. nil when the blocker is built and can simply be merged first.
    public let blockedNote: String?
    /// Why this repo has nothing to merge (it is running, conflicted or failed); nil when it is built.
    public let ownNote: String?

    /// Merge is offered only for the repo that is next in order and has something to merge.
    public var canMerge: Bool { blockedBy == nil && ownNote == nil }

    public static let blockedLead = "The set merges one repo at a time, in merge order."

    public static func make(
        _ set: IntegrationSet, verify: [RepoVerify]?, mainBranch: String = "main"
    ) -> SetMergeSheet {
        let live = set.liveMembers
        let position = live.firstIndex { $0.repo == set.repo }.map { $0 + 1 } ?? Int(set.position) + 1
        let blocker = set.blocker(of: set.repo)
        return SetMergeSheet(
            title: "Merge " + set.repo + " into " + mainBranch,
            order: "Repo \(position) of \(live.count) in merge order",
            members: SetMemberRow.rows(set, verify: verify), gate: SetGate.make(set, verify: verify),
            after: after(set), action: "Merge " + set.repo, blockedBy: blocker?.repo,
            blockedTitle: blocker == nil ? nil : set.repo + " has to wait",
            blockedNote: blocker.flatMap { notBuilt($0) }.map { $0 + " It needs a redo or a fix first." },
            ownNote: set.member.flatMap { notBuilt($0) })
    }

    /// `web is still running and has nothing to merge yet.` for a member that is not built; nil for a built one.
    static func notBuilt(_ member: SetMember) -> String? {
        switch member.state {
        case "running": return member.repo + " is still running and has nothing to merge yet."
        case "conflicted": return member.repo + " has a conflict and has nothing to merge yet."
        case "failed": return member.repo + " failed and has nothing to merge yet."
        case "confirmed": return member.repo + " is already in main."
        case "discarded": return member.repo + " was discarded and has nothing to merge."
        default: return nil
        }
    }

    /// The repo merged after this one: the next live member after it that is not in main.
    static func following(_ set: IntegrationSet) -> SetMember? {
        set.liveMembers.first { $0.position > set.position && $0.state != "confirmed" && $0.state != "discarded" }
    }

    static func after(_ set: IntegrationSet) -> String {
        guard let next = following(set) else { return "This is the last repo of the set." }
        return "After this: " + next.repo
    }
}

public enum SetMergeText {
    /// The repo named by the host's order refusal (`merge <x> first`), when the text is exactly that and the repo is a
    /// member of the set. Anything else is not read as an order refusal.
    public static func orderRepo(_ refusal: DecideRefusal, in set: IntegrationSet) -> String? {
        guard refusal.message.verified else { return nil }
        let text = refusal.message.displayPlain.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix("merge "), text.hasSuffix(" first") else { return nil }
        let repo = String(text.dropFirst("merge ".count).dropLast(" first".count))
        return set.members.contains { $0.repo == repo } ? repo : nil
    }

    /// The text of a result for a member of a set; nil when the generic text of a single-repo merge fits.
    static func make(_ outcome: DecideOutcome, set: IntegrationSet, hostName: String) -> MergeText? {
        switch outcome {
        case .refused(let refusal, _):
            if let repo = orderRepo(refusal, in: set) {
                var made = MergeText(
                    tone: .warning, title: "merge " + repo + " first", line: SetMergeSheet.blockedLead + " Nothing was merged.",
                    headline: set.repo + " has to wait", hostText: nil)
                made.reason = "merge " + repo + " first"
                made.openRepo = repo
                return made
            }
            if case .other(let wire) = refusal.code, wire == "merge_unrecorded" { return unrecorded(set) }
            return nil
        case .applied(let result), .forwarded(let result):
            guard let merge = result.merge else { return nil }
            let sha = merge.mergeCommit.map { String($0.prefix(7)) }
            var made = MergeText(
                tone: .good, title: set.repo + " is in main" + (sha.map { " · " + $0 } ?? ""),
                line: set.repo + " is confirmed. The PM was told.",
                headline: set.repo + " is in main" + (sha.map { " · " + $0 } ?? ""), hostText: nil)
            made.nextRepo = SetMergeSheet.following(set)?.repo
            return made
        default:
            return nil
        }
    }

    private static func unrecorded(_ set: IntegrationSet) -> MergeText {
        var made = MergeText(
            tone: .warning, title: "Confirm " + set.repo + " from the terminal",
            line: "The merge commit is on main. Recording it failed, so "
                + (SetMergeSheet.following(set)?.repo ?? "the next repo") + " waits until " + set.repo + " is confirmed.",
            headline: "Confirm " + set.repo + " from the terminal", hostText: nil)
        made.command = "cstan integrate confirm " + set.setId + " --repo " + set.repo
        made.nextRepo = SetMergeSheet.following(set)?.repo
        return made
    }
}
