import Foundation
@testable import CapstanKit

/// Answers of `cstan recent --json`, written out the way the command prints them.
enum RecentSamples {
    static let epoch = "daeca3b2245f3519"

    static func event(seq: Int, k: Int = 0, type: String = "report.recorded", sha: String = "1111111111111111111111111111111111111111") -> String {
        let id = "pfixture:\(epoch):\(seq)" + (k == 0 ? "" : ":\(k)")
        let head = "{\"id\":\"\(id)\",\"seq\":\(seq),\"k\":\(k),\"ts\":\"2026-01-01T00:00:00.000Z\",\"type\":\"\(type)\","
        let tail = "\"actorId\":\"dev-1\",\"entity\":{\"type\":\"agent_report\",\"id\":\"r\(seq)\"},\"from\":null,\"to\":null,"
        let body = "\"body\":{\"effects\":[{\"section\":\"waiting\",\"op\":\"invalidate\",\"key\":\"message\",\"fields\":{}}],"
        let rest = "\"agentId\":\"dev-1\",\"commitSha\":\"\(sha)\"}}"
        return [head, tail, body, rest].joined()
    }

    static func page(events: [String], head: Int = 9, truncated: Bool = false, projectId: String = "pfixture") -> String {
        let list = events.joined(separator: ",")
        let first = "{\"projectId\":\"\(projectId)\",\"epoch\":\"\(epoch)\",\"head\":\(head),\"events\":[\(list)],"
        let last = "\"oldestScanned\":1,\"truncated\":\(truncated),\"rowsRead\":3,\"rowsScanned\":3}"
        return first + last
    }

    static func feedEvent(seq: Int, k: Int = 0, type: String = "report.recorded", sha: String = "1111111111111111111111111111111111111111") throws -> FeedEvent {
        let value = try JSONDecoder().decode(JSONValue.self, from: Data(event(seq: seq, k: k, type: type, sha: sha).utf8))
        return try FeedEvent(json: value)
    }

    static func page(_ events: [FeedEvent], truncated: Bool = false, head: Int64 = 9) -> RecentPage {
        RecentPage(projectId: "pfixture", epoch: epoch, head: head, events: events, truncated: truncated)
    }
}
