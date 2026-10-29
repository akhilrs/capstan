#if DEBUG
import CapstanAppCore
import CapstanKit
import Foundation

/// Models for the previews of this folder: one project with an item of every kind, in the extremes the design lists.
@MainActor
enum InboxPreviewFixtures {
    static let host = PreviewData.alphaId
    static let projectId = "capstan"

    static func rowID(_ key: String) -> InboxRowID {
        InboxRowID(hostId: host, projectId: projectId, key: key)
    }

    private static let since = "2026-10-29T08:00:00Z"
    private static let long = String(repeating: "0123456789abcdef", count: 256)

    private static func stored(
        _ key: String, kind: WaitingKind, summary: String, verified: Bool = true, needsTerminal: Bool = false
    ) -> StoredWaiting {
        StoredWaiting(
            key: key, requestId: "req-" + key, kind: kind, since: since,
            summary: UntrustedText(raw: summary, verified: verified, truncated: false), contentHash: "aaaaaaaaaaaa",
            needsTerminal: needsTerminal)
    }

    private static func detail(_ key: String, _ type: String, _ fields: [String: JSONValue]) -> ItemDetail {
        var body = fields
        body["effects"] = .array([])
        return ItemDetail(key: key, eventType: type, seq: 5, body: .object(body))
    }

    private static func option(_ number: Int64, _ text: String, text accepts: Bool = false, widens: Bool = false) -> JSONValue {
        .object([
            "number": .int(number), "text": .string(text), "acceptsText": .bool(accepts), "widensPermissions": .bool(widens),
        ])
    }

    private static func opDetail(_ key: String, command: String, verified: Bool = true) -> ItemDetail {
        detail(key, "operator.proposed", [
            "proposalId": .string("p1"), "kind": .string("command"), "state": .string("waiting"),
            "command": .string(command), "reason": .string(""), "textVerified": .bool(verified),
        ])
    }

    private static func messageDetail(_ key: String, body: String, verified: Bool = true) -> ItemDetail {
        detail(key, "message.waiting", [
            "messageId": .string("m1"), "recipientAgentId": .string("pm-1"), "state": .string("waiting"),
            "body": .string(body), "reason": .string(""), "textVerified": .bool(verified),
        ])
    }

    static func seed(decide: Bool = true, stale: Bool = false) -> PreviewSeed {
        var seed = PreviewData.standard
        let items = [
            stored("op", kind: .op, summary: "git push origin feat/plan-92"),
            stored("op-long", kind: .op, summary: "A very long command"),
            stored("op-bad", kind: .op, summary: "", verified: false),
            stored("op-nodetail", kind: .op, summary: "git push origin feat/plan-92"),
            stored("op-terminal", kind: .op, summary: "Needs a terminal", needsTerminal: true),
            stored("prompt", kind: .prompt, summary: "Allow edits to Package.swift?"),
            stored("prompt-terminal", kind: .prompt, summary: "Allow edits to Package.swift?", needsTerminal: true),
            stored("design", kind: .design, summary: "Approve the Mac app design"),
            stored("message", kind: .message, summary: "Which target should the merge use?"),
            stored("message-long", kind: .message, summary: "A long message"),
            stored("message-bad", kind: .message, summary: "", verified: false),
            stored("merge", kind: .integration, summary: "feat/plan-92 is ready to merge"),
        ]
        let promptFields: [String: JSONValue] = [
            "relayId": .string("r1"), "agentId": .string("dev-needs"), "state": .string("waiting"),
            "text": .string("Allow edits to Package.swift?"), "reason": .string(""),
            "options": .array([
                option(1, "Yes"), option(2, "Yes, and do not ask again", widens: true), option(3, "No, tell the agent what to do", text: true),
            ]),
        ]
        let details = [
            opDetail("op", command: "git push origin feat/plan-92"),
            opDetail("op-long", command: long),
            opDetail("op-bad", command: "", verified: false),
            opDetail("op-terminal", command: "make deploy"),
            detail("prompt", "prompt.waiting", promptFields),
            detail("prompt-terminal", "prompt.waiting", promptFields),
            detail("design", "design.decided", [
                "planId": .string("plan-1"), "packageId": .string("pkg-1"), "reportId": .string("rep-1"),
                "decision": .string("waiting"), "link": .string("https://example.org/design/42"),
            ]),
            messageDetail("message", body: "Which target should the merge use? The release branch or main?"),
            messageDetail("message-long", body: long),
            messageDetail("message-bad", body: "", verified: false),
        ]
        let project = ProjectModel(
            projectId: projectId, root: "/work/capstan", availability: stale ? .silent : .ready, head: 42,
            waiting: Dictionary(uniqueKeysWithValues: items.map { ($0.key, $0) }),
            details: Dictionary(uniqueKeysWithValues: details.map { ($0.key, $0) }), snapshotComplete: true,
            staleSince: stale ? Date().addingTimeInterval(-300) : nil)
        seed.hostModels[host] = HostModel(
            hostId: host, hello: nil, projects: [projectId: project],
            forwards: [projectId: [ForwardRecord(requestId: "req-op", contentHash: "aaaaaaaaaaaa", at: Date())]])
        seed.capabilities[host] = HostCapabilities(reachable: true, events: true, decide: decide)
        return seed
    }

    static func model(decide: Bool = true, stale: Bool = false) -> AppModel {
        AppModel(preview: seed(decide: decide, stale: stale))
    }

    /// A model with no waiting items at all.
    static func emptyModel() -> AppModel {
        var seed = PreviewData.standard
        for id in seed.hostModels.keys { seed.hostModels[id]?.projects = [:] }
        return AppModel(preview: seed)
    }
}
#endif
