import CapstanAppCore
import CapstanKit
import Foundation

/// The words of a merge's result (board 10). Ids, codes and times only; a refusal's host text (the conflicting files,
/// git's reason) is shown apart as untrusted text. **There is no push**: the host never pushes a merge (D8), so there is
/// no push result and no "push failed".
struct MergeText: Equatable {
    enum Tone: Equatable { case good, warning, bad }

    let tone: Tone
    let title: String
    let line: String
    /// The screen title after the result.
    let headline: String
    /// Host text to show under the line, as untrusted (the files of a conflict, git's reason).
    let hostText: UntrustedText?

    static func make(_ outcome: DecideOutcome, branch: String, hostName: String, at: Date) -> MergeText {
        let time = at.formatted(date: .omitted, time: .shortened)
        switch outcome {
        case .applied(let result), .forwarded(let result):
            guard let merge = result.merge else {
                return MergeText(
                    tone: .good, title: "Merged", line: "The host merged the branch. " + time, headline: branch + " is merged",
                    hostText: nil)
            }
            if merge.alreadyInHead {
                return MergeText(
                    tone: .good, title: "Already in the branch, confirmed",
                    line: "The project's branch already had this integration; it is now confirmed. " + time,
                    headline: merge.branch + " is merged", hostText: nil)
            }
            let commit = merge.mergeCommit.map { " as " + String($0.prefix(12)) } ?? ""
            return MergeText(
                tone: .good, title: "Merged, not pushed",
                line: "Merged into the project's branch" + commit + " at " + time + ". The PM was told."
                    + (merge.cleanupScheduled ? " The integration branch is cleaned up." : ""),
                headline: merge.branch + " is merged", hostText: nil)
        case .refused(let refusal, _):
            return refused(refusal, hostName: hostName)
        case .notSent(let reason, _):
            if case .timeout = reason {
                return MergeText(
                    tone: .warning, title: "No answer yet",
                    line: "The merge may still be running on " + hostName + ". Retry asks again; it never merges twice.",
                    headline: "Merge not confirmed", hostText: nil)
            }
            return MergeText(
                tone: .bad, title: "Could not reach " + hostName, line: "Nothing was merged. Retry asks again.",
                headline: "Nothing was merged", hostText: nil)
        case .malformed:
            return MergeText(
                tone: .warning, title: "Unexpected answer",
                line: "The host's answer could not be read; the merge may have been made. Retry asks again safely.",
                headline: "Merge not confirmed", hostText: nil)
        }
    }

    private static func refused(_ refusal: DecideRefusal, hostName: String) -> MergeText {
        let code: String
        switch refusal.code {
        case .other(let wire): code = wire
        case .notWaiting: code = "not_waiting"
        case .hashMismatch: code = "hash_mismatch"
        default: code = "other"
        }
        let host = refusal.message.raw.isEmpty ? nil : refusal.message
        func refused(_ title: String, _ line: String, files: Bool = false) -> MergeText {
            MergeText(
                tone: .warning, title: "Refused: " + title, line: line + " Nothing was merged.",
                headline: "Nothing was merged", hostText: files ? host : nil)
        }
        switch code {
        case "merge_conflict":
            return MergeText(
                tone: .bad, title: "The merge has conflicts",
                line: "Nothing changed in the checkout on " + hostName + ". The PM can rebase the integration and ask again.",
                headline: "Conflicts, nothing was merged", hostText: host)
        case "merge_disabled":
            return refused(
                "merging from the phone is off",
                "Turn it on with [remote] merge = true in the project's Capstan configuration on " + hostName + ".")
        case "run_paused": return refused("the project is paused", "Resume the project first.")
        case "busy": return refused("an integration is running", "Try again when it is done.")
        case "not_reviewed": return refused("not reviewed", "The integration's latest review has not passed.")
        case "not_verified": return refused("not verified", "The integration has no passed verify on this commit.")
        case "branch_moved":
            return refused("the branch moved", "The integration branch changed after the integration was made.")
        case "wrong_branch":
            return refused("wrong branch", "The checkout on " + hostName + " is not on the branch the merge goes into.")
        case "dirty_tree":
            return refused("the working tree has changes", "Tracked files are modified in the checkout on " + hostName + ".", files: true)
        case "unsafe_config": return refused("unsafe git configuration", "The repository's git settings are not safe for a merge.")
        case "no_identity": return refused("no git identity", "git has no committer name and email in the repository.")
        case "git_too_old": return refused("git is too old", "Update git on " + hostName + ".")
        case "invalid_commit", "invalid_branch":
            return refused("not a valid commit or branch", "The integration names something git does not accept.")
        case "git_unavailable": return refused("git could not run", "git is not available on " + hostName + ".", files: true)
        case "merge_failed":
            return MergeText(
                tone: .bad, title: "The merge failed", line: "git stopped during the merge. The host's reason is below.",
                headline: "The merge failed", hostText: host)
        case "merge_unrecorded":
            return MergeText(
                tone: .warning, title: "In the branch, not confirmed",
                line: "The merge worked but Capstan could not record it. Retry records it; or the PM confirms it.",
                headline: "Merged, not confirmed", hostText: nil)
        case "not_waiting":
            return MergeText(
                tone: .warning, title: "No longer waiting", line: "The integration was merged or withdrawn already.",
                headline: "Nothing to merge", hostText: nil)
        case "hash_mismatch":
            return MergeText(
                tone: .warning, title: "This changed since you opened it",
                line: "The integration changed on the host. Nothing was merged; read it again.",
                headline: "Nothing was merged", hostText: nil)
        default:
            return MergeText(
                tone: .bad, title: "Refused", line: "The host refused the merge. Nothing was merged.",
                headline: "Nothing was merged", hostText: host)
        }
    }
}
