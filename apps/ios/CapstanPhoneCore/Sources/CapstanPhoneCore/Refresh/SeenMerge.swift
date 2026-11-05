import Foundation

/// How the seen set of one pass joins the one stored before it, pure. Keys of a host read completely in the pass are
/// replaced by what the pass found; keys of any other host are kept. Item keys start with `host-uuid/`, stuck-episode
/// keys with `pmstuck/host-uuid/`, so both prefixes of every complete host are read.
public enum SeenMerge {
    public static func merged(previous: Set<String>?, pass: Set<String>, completeHostIds: [UUID]) -> Set<String> {
        let readPrefixes = completeHostIds.flatMap { id -> [String] in
            let host = id.uuidString
            return ["\(host)/", "\(RefreshPlanner.pmStuckPrefix)\(host)/"]
        }
        let kept = (previous ?? []).filter { key in !readPrefixes.contains { key.hasPrefix($0) } }
        return pass.union(kept)
    }
}
