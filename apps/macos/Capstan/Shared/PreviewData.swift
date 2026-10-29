import CapstanAppCore
import CapstanKit
import Foundation

/// Fixtures for #Previews and tests, made by decoding JSON text because the kit's memberwise initializers are internal.
/// A fixture that fails to decode is skipped, so a preview never crashes; `decoded` and `written` count them.
@MainActor
enum PreviewData {
    static let alphaId = UUID(uuidString: "A1A1A1A1-0000-4000-8000-000000000001")!
    static let betaId = UUID(uuidString: "B2B2B2B2-0000-4000-8000-000000000002")!

    /// The number of fixtures written below and the number that decoded; they are equal when nothing is skipped.
    private(set) static var written = 0
    private(set) static var decoded = 0

    private static func decode<T: Decodable>(_ type: T.Type, _ json: String) -> T? {
        written += 1
        let value = try? JSONDecoder().decode(type, from: Data(json.utf8))
        if value != nil { decoded += 1 }
        return value
    }

    private static func text(_ raw: String, verified: Bool = true, truncated: Bool = false) -> String {
        let escaped = raw.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "{\"raw\":\"\(escaped)\",\"verified\":\(verified),\"truncated\":\(truncated)}"
    }

    private static func waiting(
        _ key: String, kind: String, summary: String, hash: String, needsTerminal: Bool = false
    ) -> String {
        "\"\(key)\":{\"key\":\"\(key)\",\"requestId\":\"req-\(hash)\",\"kind\":\"\(kind)\",\"since\":\"2026-10-29T08:00:00Z\","
            + "\"summary\":\(summary),\"contentHash\":\"\(hash)\",\"needsTerminal\":\(needsTerminal)}"
    }

    private static func agent(_ id: String, role: String, state: String, paused: Bool = false, task: String? = "Write the shared views") -> String {
        let taskPart = task.map { ",\"task\":\(text($0))" } ?? ""
        return "\"agent:\(id)\":{\"key\":\"agent:\(id)\",\"agentId\":\"\(id)\",\"kind\":\"claude\",\"role\":\"\(role)\","
            + "\"generation\":1,\"state\":\"\(state)\",\"paused\":\(paused)\(taskPart),\"requestId\":\"req-\(id)\",\"contentHash\":\"h-\(id)\"}"
    }

    private static func plan(_ id: String, seq: Int, title: String, state: String, counts: (Int, Int, Int, Int)) -> String {
        "\"plan:\(id)\":{\"key\":\"plan:\(id)\",\"planId\":\"\(id)\",\"sequence\":\(seq),\"title\":\(text(title)),\"state\":\"\(state)\","
            + "\"packages\":{\"total\":\(counts.0),\"assigned\":\(counts.1),\"reported\":\(counts.2),\"integrated\":\(counts.3)}}"
    }

    private static func project(
        _ id: String, root: String, availability: String, waiting: String = "", agents: String = "", plans: String = "",
        details: String = "", attention: String = ""
    ) -> String {
        "\"\(id)\":{\"projectId\":\"\(id)\",\"root\":\"\(root)\",\"availability\":\"\(availability)\",\"epoch\":\"e1\",\"head\":42,"
            + "\"waiting\":{\(waiting)},\"agents\":{\(agents)},\"plans\":{\(plans)},\"run\":null,\"details\":{\(details)},"
            + "\"attention\":{\(attention)},\"snapshotComplete\":true,\"staleSince\":null}"
    }

    private static func host(_ id: UUID, name: String, destination: String) -> HostConfig? {
        decode(HostConfig.self,
            "{\"id\":\"\(id.uuidString)\",\"displayName\":\"\(name)\",\"destination\":\"\(destination)\",\"cstanPath\":\"cstan\","
                + "\"extraRoots\":[],\"pollMs\":2000,\"identityFile\":null,\"enabled\":true}")
    }

    private static func hostModel(_ id: UUID, hello: String, projects: String) -> HostModel? {
        decode(HostModel.self,
            "{\"hostId\":\"\(id.uuidString)\",\"hello\":{\"version\":\(text("1")),\"host\":\(text(hello)),\"projects\":3},"
                + "\"projects\":{\(projects)},\"cursor\":null,\"forwards\":{}}")
    }

    /// Two hosts with every availability; a waiting item of each kind (one of 4096 bytes, one failing integrity);
    /// agents in every health; plans at several stages.
    static var standard: PreviewSeed {
        written = 0
        decoded = 0
        let long = text(String(repeating: "0123456789abcdef", count: 256), truncated: true)
        let unverified = text("", verified: false)
        let promptDetail = "\"waiting-prompt\":{\"key\":\"waiting-prompt\",\"eventType\":\"prompt.waiting\",\"seq\":7,\"body\":"
            + "{\"relayId\":\"r1\",\"agentId\":\"dev-needs\",\"state\":\"waiting\",\"text\":\"Allow edits to Package.swift?\","
            + "\"options\":[{\"number\":1,\"text\":\"Yes\",\"acceptsText\":false,\"widensPermissions\":false},"
            + "{\"number\":2,\"text\":\"Yes, and do not ask again\",\"acceptsText\":false,\"widensPermissions\":true}],"
            + "\"textVerified\":true}}"
        let alphaWaiting = [
            waiting("waiting-op", kind: "op", summary: text("git push origin feat/plan-92"), hash: "a1"),
            waiting("waiting-prompt", kind: "prompt", summary: text("Allow edits to Package.swift?"), hash: "a2"),
            waiting("waiting-design", kind: "design", summary: text("Approve the Mac app design"), hash: "a3"),
            waiting("waiting-message", kind: "message", summary: text("Which target should the merge use?"), hash: "a4"),
            waiting("waiting-merge", kind: "integration", summary: text("feat/plan-92 is ready to merge"), hash: "a5"),
            waiting("waiting-long", kind: "op", summary: long, hash: "a6", needsTerminal: true),
            waiting("waiting-unverified", kind: "message", summary: unverified, hash: "a7"),
        ].joined(separator: ",")
        let agents = [
            agent("dev-working", role: "developer", state: "active"),
            agent("dev-needs", role: "developer", state: "active"),
            agent("dev-stalled", role: "developer", state: "active"),
            agent("dev-lost", role: "developer", state: "lost"),
            agent("dev-blocked", role: "developer", state: "blocked"),
            agent("dev-paused", role: "developer", state: "active", paused: true),
            agent("dev-idle", role: "reviewer", state: "idle", task: nil),
        ].joined(separator: ",")
        let plans = [
            plan("p-start", seq: 1, title: "Not started", state: "approved", counts: (4, 0, 0, 0)),
            plan("p-assigned", seq: 2, title: "Assigned", state: "running", counts: (4, 3, 0, 0)),
            plan("p-reported", seq: 3, title: "In review", state: "running", counts: (4, 4, 3, 1)),
            plan("p-done", seq: 4, title: "Integrated", state: "done", counts: (4, 4, 4, 4)),
        ].joined(separator: ",")
        let alphaProjects = [
            project("capstan", root: "/work/capstan", availability: "ready", waiting: alphaWaiting, agents: agents,
                plans: plans, details: promptDetail, attention: "\"dev-stalled\":\"stalled\""),
            project("notes", root: "/work/notes", availability: "no_key"),
            project("site", root: "/work/site", availability: "silent"),
        ].joined(separator: ",")
        let betaProjects = [
            project("api", root: "/srv/api", availability: "refused"),
            project("legacy", root: "/srv/legacy", availability: "unsupported"),
            project("old", root: "/srv/old", availability: "gone"),
        ].joined(separator: ",")

        let hosts = [
            host(alphaId, name: "Studio", destination: "studio.local"),
            host(betaId, name: "Build box", destination: "build@10.0.0.7"),
        ].compactMap { $0 }
        var models: [UUID: HostModel] = [:]
        if let m = hostModel(alphaId, hello: "studio", projects: alphaProjects) { models[alphaId] = m }
        if let m = hostModel(betaId, hello: "build-box", projects: betaProjects) { models[betaId] = m }
        let now = Date()
        return PreviewSeed(
            hosts: hosts, hostModels: models,
            connection: [
                alphaId: .live(since: now.addingTimeInterval(-600), lastLineAt: now),
                betaId: .reconnecting(attempt: 2, retryAt: now.addingTimeInterval(8), lastError: .interrupted),
            ],
            capabilities: [
                alphaId: HostCapabilities(reachable: true, events: true, decide: true),
                betaId: HostCapabilities(reachable: true, events: true, decide: false),
            ])
    }
}

#if DEBUG
import SwiftUI

#Preview("PreviewData fixtures") {
    let seed = PreviewData.standard
    let line = String(PreviewData.decoded) + " of " + String(PreviewData.written) + " fixtures decoded, "
        + String(seed.hosts.count) + " hosts"
    Text(verbatim: line).padding()
}
#endif
