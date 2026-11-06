import CapstanKit
import Foundation
@testable import CapstanPhoneCore

enum VoiceFixture {
    static let hostA = UUID(uuidString: "00000000-0000-0000-0000-00000000000A")!
    static let hostB = UUID(uuidString: "00000000-0000-0000-0000-00000000000B")!
    static let now = ISO8601DateFormatter().date(from: "2026-11-05T12:00:00Z")!
    static let hash = "abcdef123456"

    static func text(_ raw: String, verified: Bool = true, truncated: Bool = false) -> UntrustedText {
        UntrustedText(raw: raw, verified: verified, truncated: truncated)
    }

    static func everything(prefixes: [String] = ["git push", "git push origin"]) -> ApproveChoices {
        ApproveChoices(
            once: true, exact: true, prefixes: prefixes.map { text($0) }, auto: [.hour, .session])
    }

    static func item(
        _ key: String = "k1", command: String = "git push origin main", since: String = "2026-11-05T10:00:00Z",
        kind: WaitingKind = .op, choices: ApproveChoices? = everything(), verified: Bool = true,
        truncated: Bool = false, needsTerminal: Bool = false, hash: String = hash
    ) -> StoredWaiting {
        StoredWaiting(
            key: key, requestId: "op:" + key, kind: kind, since: since,
            summary: text(command, verified: verified, truncated: truncated), contentHash: hash,
            needsTerminal: needsTerminal, approveChoices: choices)
    }

    static func host(
        _ id: UUID = hostA, _ name: String = "build-host", project: String = "payments-api",
        projectId: String = "p1", items: [StoredWaiting]
    ) -> VoiceHostModel {
        var waiting: [String: StoredWaiting] = [:]
        for item in items { waiting[item.key] = item }
        let model = ProjectModel(projectId: projectId, root: "/srv/" + project, waiting: waiting)
        return VoiceHostModel(id: id, name: name, model: HostModel(hostId: id, projects: [projectId: model]))
    }

    static func settings(decisions: Bool = true, grants: Bool = true, label: String = "iPhone") -> PhoneSettings {
        PhoneSettings(clientLabel: label, voiceDecisions: decisions, voiceGrants: grants)
    }

    static func target(_ item: StoredWaiting = item()) -> VoiceTarget {
        VoiceDesk.target([host(items: [item])])!
    }

    static func applied(verb: String, approval: ApprovalResult? = nil) -> DecideOutcome {
        .applied(DecideResult(
            decisionId: "d1", requestId: "op:k1", nonce: "n", outcome: .applied, isRepeat: false, kind: "op",
            verb: verb, contentHash: hash, approval: approval))
    }
}
