import CapstanAppCore
import CapstanKit
import Foundation

/// Variants of `PreviewData.standard` for the previews of the host and connection views. Nothing here runs a session.
@MainActor
enum HostsPreviewFixtures {
    private static func note(_ raw: String) -> UntrustedText {
        UntrustedText(raw: raw, verified: true, truncated: false)
    }

    /// Every connection state, the stopped ones with each reason and the reconnecting ones with each error.
    static var connections: [(name: String, state: ConnectionState)] {
        let now = Date()
        let retry = now.addingTimeInterval(20)
        return [
            ("connecting", .connecting),
            ("live", .live(since: now.addingTimeInterval(-600), lastLineAt: now)),
            ("no heartbeat", .noHeartbeat(since: now.addingTimeInterval(-45))),
            ("reconnecting, interrupted", .reconnecting(attempt: 1, retryAt: retry, lastError: .interrupted)),
            ("reconnecting, ssh", .reconnecting(
                attempt: 2, retryAt: retry, lastError: .ssh(tail: note("Permission denied (publickey)")))),
            ("reconnecting, stream", .reconnecting(
                attempt: 2, retryAt: retry, lastError: .stream(code: "unknown_verb", message: note("no such verb")))),
            ("reconnecting, exit", .reconnecting(
                attempt: 3, retryAt: retry, lastError: .exit(137, tail: note("Killed")))),
            ("stopped by the user", .stopped(.user)),
            ("stopped, old app", .stopped(.unsupportedVersion(2))),
            ("stopped, host refused", .stopped(.hostRejected(code: "forbidden", tail: note("not allowed")))),
            ("stopped, cstan missing", .stopped(.cstanMissing(tail: note("bash: /nonexistent/cstan: No such file")))),
            ("stopped, repeated failure", .stopped(.repeatedFailure)),
            ("stopped, unverified text", .stopped(.hostRejected(
                code: nil, tail: UntrustedText(raw: "", verified: false, truncated: false)))),
        ]
    }

    /// The standard seed with the first host in `connection`.
    static func seed(connection: ConnectionState, decide: Bool = true) -> PreviewSeed {
        var seed = PreviewData.standard
        seed.connection[PreviewData.alphaId] = connection
        seed.capabilities[PreviewData.alphaId] = HostCapabilities(reachable: true, events: true, decide: decide)
        return seed
    }

    /// The standard seed with every project of the first host made `availability`.
    static func seed(availability: ProjectAvailability) -> PreviewSeed {
        var seed = PreviewData.standard
        if var host = seed.hostModels[PreviewData.alphaId] {
            for id in host.projects.keys { host.projects[id]?.availability = availability }
            seed.hostModels[PreviewData.alphaId] = host
        }
        return seed
    }

    /// Check results: each step passed, one warning, one failure per step and an ssh refusal.
    static let checkRuns: [(name: String, checks: [HostCheck])] = [
        ("all passed", [
            HostCheck(step: .connect, status: .passed, text: "Connected"),
            HostCheck(step: .capstan, status: .passed, text: "cstan found"),
            HostCheck(step: .projects, status: .passed, text: "3 projects"),
            HostCheck(step: .firstSnapshot, status: .passed, text: "Snapshot read"),
        ]),
        ("warning", [
            HostCheck(step: .connect, status: .passed, text: "Connected"),
            HostCheck(step: .capstan, status: .passed, text: "cstan found"),
            HostCheck(step: .projects, status: .passed, text: "3 projects"),
            HostCheck(step: .firstSnapshot, status: .warning, text: "No snapshot within 10 s"),
        ]),
        ("connect failed", [
            HostCheck(step: .connect, status: .failed, text: "ssh: Permission denied (publickey)"),
        ]),
        ("cstan missing", [
            HostCheck(step: .connect, status: .passed, text: "Connected"),
            HostCheck(step: .capstan, status: .failed, text: "cstan was not found"),
        ]),
        ("no projects", [
            HostCheck(step: .connect, status: .passed, text: "Connected"),
            HostCheck(step: .capstan, status: .passed, text: "cstan found"),
            HostCheck(step: .projects, status: .failed, text: "No projects"),
        ]),
        ("in progress", [
            HostCheck(step: .connect, status: .passed, text: "Connected"),
        ]),
    ]

    /// A config that fails exactly the named check of `HostConfig.validate()`.
    static let invalidConfigs: [(name: String, config: HostConfig)] = [
        ("destination", HostConfig(displayName: "Studio", destination: "-oProxyCommand=x")),
        ("cstan path", HostConfig(displayName: "Studio", destination: "studio.local", cstanPath: "cstan; ls")),
        ("project folder", HostConfig(
            displayName: "Studio", destination: "studio.local", extraRoots: ["relative/dir"])),
        ("poll interval", HostConfig(displayName: "Studio", destination: "studio.local", pollMs: 10)),
        ("poll interval, too long", HostConfig(
            displayName: "Studio", destination: "studio.local", pollMs: HostLimits.maxPollMs + 1)),
        ("key file", HostConfig(displayName: "Studio", destination: "studio.local", identityFile: "id_ed25519")),
    ]
}
