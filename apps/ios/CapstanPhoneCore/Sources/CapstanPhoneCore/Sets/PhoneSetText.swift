import CapstanAppCore
import CapstanKit
import Foundation

/// The iPhone's reading of an integration set (plan-185, board 08). The words, glyphs and sentences are
/// `CapstanAppCore.SetText`'s, shared with the Mac; only the strip cap differs: the phone row shows 8 glyphs, then `+n`.
public enum PhoneSetText {
    /// Glyphs the inbox row's strip shows before `+n` on the phone.
    public static let stripCap = 8

    /// The second line of a set row: the strip, then why the set waits (`Merge web next`).
    public static func secondary(_ set: IntegrationSet) -> String {
        let strip = SetText.strip(set, cap: stripCap).text
        guard let reason = SetText.reason(set) else { return strip }
        return strip + "  " + reason
    }

    /// The strip as one VoiceOver element: the summary, the first members, then "and n more repos".
    public static func stripVoiceover(_ set: IntegrationSet) -> String {
        SetText.stripVoiceover(set, cap: stripCap)
    }

    /// The set an item is a member of, from the feed's `set` or the details the inspector reads; nil for an item of a
    /// project without `[repos]`, whose rows and sheet stay as they were.
    public static func set(of item: StoredWaiting) -> IntegrationSet? {
        item.set ?? item.integrationDetails?.set
    }

    /// The key of the waiting item of another repo of the same set, when the host holds one: "Open <repo>" and
    /// "Merge <next>…" are offered only for a repo that has an item to show.
    public static func waitingKey(of repo: String, in set: IntegrationSet, among waiting: [StoredWaiting]) -> String? {
        SetText.waitingKey(of: repo, in: set, among: waiting)
    }
}

/// What a set's merge sheet offers, from its text. The sheet decides one member at a time.
public enum PhoneSetMergeOffer: Equatable, Sendable {
    /// Merge this repo (it is next and built).
    case merge
    /// Another repo has to be merged first; `open` is that repo when it has a waiting item, else nil (the note stands
    /// with Close alone).
    case blocked(open: String?)
    /// This repo has nothing to merge (running, conflicted, failed, confirmed or discarded): no Merge.
    case nothingToMerge

    public static func make(_ sheet: SetMergeSheet, waitingKeyOf: (String) -> String?) -> PhoneSetMergeOffer {
        if let by = sheet.blockedBy { return .blocked(open: waitingKeyOf(by) == nil ? nil : by) }
        return sheet.canMerge ? .merge : .nothingToMerge
    }
}
