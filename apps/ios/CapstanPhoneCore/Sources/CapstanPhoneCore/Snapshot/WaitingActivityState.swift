import CapstanKit
import Foundation

/// What the "N waiting" Live Activity shows (board 15): counts by kind, how many hosts are live, and when.
/// Counts and times only: no project name and nothing a host or an agent wrote.
public struct WaitingActivityState: Codable, Hashable, Sendable {
    public var total: Int
    public var proposals: Int
    public var prompts: Int
    public var designs: Int
    public var messages: Int
    public var merges: Int
    public var asOf: Date
    public var hostsLive: Int
    public var hostsTotal: Int

    public init(_ snapshot: GlanceSnapshot) {
        total = snapshot.totalWaiting
        proposals = snapshot.count(of: .op)
        prompts = snapshot.count(of: .prompt)
        designs = snapshot.count(of: .design)
        messages = snapshot.count(of: .message)
        merges = snapshot.count(of: .integration)
        asOf = snapshot.asOf
        hostsLive = snapshot.hostsReachable
        hostsTotal = snapshot.hostsTotal
    }

    /// "2 proposals · 2 prompts · 1 design · 1 message · 1 merge", leaving out kinds at zero.
    public var kindsLine: String { parts.joined(separator: " · ") }

    /// The first `keep` kinds and "N more" for the rest, for the Lock Screen: "2 proposals · 2 prompts · 3 more".
    public func shortLine(keep: Int = 2) -> String {
        let all = parts
        guard all.count > keep else { return all.joined(separator: " · ") }
        let rest = counts.dropFirst(keep).map(\.count).reduce(0, +)
        return (all.prefix(keep) + [String(rest) + " more"]).joined(separator: " · ")
    }

    /// "1 of 2 hosts live", or "live" with one host.
    public var hostsLine: String {
        if hostsTotal <= 1 { return hostsLive == hostsTotal ? "live" : "not live" }
        return String(hostsLive) + " of " + String(hostsTotal) + " hosts live"
    }

    private var counts: [(count: Int, one: String, many: String)] {
        [
            (proposals, "proposal", "proposals"), (prompts, "prompt", "prompts"), (designs, "design", "designs"),
            (messages, "message", "messages"), (merges, "merge", "merges"),
        ].filter { $0.0 > 0 }.map { (count: $0.0, one: $0.1, many: $0.2) }
    }

    private var parts: [String] {
        counts.map { String($0.count) + " " + ($0.count == 1 ? $0.one : $0.many) }
    }
}
