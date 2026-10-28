import CapstanKit
import Foundation
import XCTest

enum ShellFixtures {
    static let hostA = UUID(uuidString: "11111111-1111-1111-1111-111111111111")!
    static let hostB = UUID(uuidString: "22222222-2222-2222-2222-222222222222")!
    static let hashA = "aaaaaaaaaaaa"
    static let hashB = "bbbbbbbbbbbb"

    static func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    static func waiting(key: String, kind: String = "op", requestId: String? = nil, hash: String = hashA) throws
        -> StoredWaiting
    {
        try decode(
            StoredWaiting.self,
            """
            {"key":"\(key)","requestId":"\(requestId ?? key)","kind":"\(kind)","since":"2026-10-01T10:00:00Z",
             "summary":{"raw":"Run tests","verified":true,"truncated":false},
             "contentHash":"\(hash)","needsTerminal":false}
            """)
    }

    static func agent(id: String, requestId: String, hash: String) throws -> StoredAgent {
        try decode(
            StoredAgent.self,
            """
            {"key":"agent:\(id)","agentId":"\(id)","kind":"developer","role":"developer","generation":1,
             "state":"working","paused":false,"requestId":"\(requestId)","contentHash":"\(hash)"}
            """)
    }

    static func run(requestId: String = "run", hash: String) throws -> StoredRun {
        try decode(
            StoredRun.self,
            "{\"requestId\":\"\(requestId)\",\"state\":\"running\",\"paused\":false,\"contentHash\":\"\(hash)\"}")
    }

    /// A project holding the given items, decoded from JSON.
    static func project(
        _ id: String = "p1", waiting: [StoredWaiting] = [], agents: [StoredAgent] = [], run: StoredRun? = nil
    ) throws -> ProjectModel {
        let encoder = JSONEncoder()
        func object<T: Encodable>(_ items: [T], key: (T) -> String) throws -> String {
            var members: [String] = []
            for item in items {
                let text = String(decoding: try encoder.encode(item), as: UTF8.self)
                members.append("\"\(key(item))\":\(text)")
            }
            return "{" + members.joined(separator: ",") + "}"
        }
        let runText = try run.map { String(decoding: try encoder.encode($0), as: UTF8.self) } ?? "null"
        return try decode(
            ProjectModel.self,
            """
            {"projectId":"\(id)","root":"/work/\(id)","availability":"ready",
             "waiting":\(try object(waiting) { $0.key }),"agents":\(try object(agents) { $0.key }),
             "plans":{},"run":\(runText),"details":{},"attention":{},"snapshotComplete":true}
            """)
    }

    static func host(_ id: UUID, projects: [ProjectModel] = []) -> HostModel {
        var map: [String: ProjectModel] = [:]
        for project in projects { map[project.projectId] = project }
        return HostModel(hostId: id, projects: map)
    }

    static func result(
        outcome: DecideKind, requestId: String = "r1", hash: String = hashA, messageId: String? = nil,
        isRepeat: Bool = false
    ) -> DecideResult {
        DecideResult(
            decisionId: "d1", requestId: requestId, nonce: "nonce-0001", outcome: outcome, isRepeat: isRepeat,
            kind: "op", verb: "approve", contentHash: hash, messageId: messageId)
    }

    static func refusal(_ code: DecideErrorCode, message: String = "no") -> DecideRefusal {
        DecideRefusal(code: code, message: UntrustedText(raw: message, verified: true, truncated: false))
    }

    static func row(host: UUID, project: String, key: String, kind: WaitingKind) -> InboxRow {
        InboxRow(
            id: InboxRowID(hostId: host, projectId: project, key: key), kind: kind, projectName: project,
            since: "2026-10-01T10:00:00Z", summary: UntrustedText(raw: "s", verified: true, truncated: false),
            needsTerminal: false, forwarded: false, stale: false, requestId: key, contentHash: hashA)
    }
}
