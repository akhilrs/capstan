import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Why one host is not live, with its fix: a problem the transport found first (a refused key, an untrusted or
/// changed host key, no common algorithm, unreadable pins), else the shared `BannerChoice` words for the stream.
struct HostBanners: View {
    @Environment(AppModel.self) private var model
    let host: HostConfig

    var body: some View {
        if let problem = model.problems[host.id] {
            problemBanner(problem)
        } else {
            let specs = BannerChoice.make(
                state: model.connection[host.id], anyLoading: false, decide: nil, hostName: host.displayName,
                destination: model.phoneHost(host.id)?.hostname ?? host.displayName, now: Date())
            ForEach(Array(specs.enumerated()), id: \.offset) { _, spec in
                banner(spec)
            }
            if case .stopped(.user)? = model.connection[host.id] {
                // Stopped by the app (the scene went to the background) and not started again.
                Banner(
                    tone: .grey, text: "Not following " + host.displayName, actionTitle: "Try again",
                    action: { model.restart(hostId: host.id) })
            }
        }
    }

    private func problemBanner(_ problem: HostProblem) -> Banner {
        let name = host.displayName
        switch problem {
        case .hostKeyUnknown:
            return Banner(
                tone: .yellow, text: name + "'s host key is not trusted yet", actionTitle: "Review the key",
                action: { model.navigation.sheet = .trust(host.id) })
        case .hostKeyChanged:
            return Banner(
                tone: .red,
                text: name + "'s host key changed, so connections are blocked. Compare the fingerprint on the host, "
                    + "then remove the host and add it again.")
        case .authRefused:
            var line: UntrustedText?
            if case .present(_, let publicLine, _) = model.key {
                line = UntrustedText(raw: publicLine, verified: true, truncated: false)
            }
            return Banner(
                tone: .red,
                text: name + " refused this iPhone's key. Add this line to ~/.ssh/authorized_keys on the host, then "
                    + "try again.",
                detail: line, actionTitle: "Try again", action: { model.restart(hostId: host.id) })
        case .unsupported(let reason):
            let text: String
            switch reason {
            case .hostKeyType(let type):
                text = name + " uses a host key type this app cannot check (" + TextBudget.displayPrefix(type, maxBytes: 64).text + ")"
            case .noCommonAlgorithm:
                text = name + " offers no SSH algorithm this app can use"
            }
            return Banner(tone: .red, text: text, actionTitle: "Try again", action: { model.restart(hostId: host.id) })
        case .pinsUnreadable:
            return Banner(
                tone: .red, text: "The saved host keys cannot be read, so every host waits", actionTitle: "Reset all host keys…",
                action: { model.navigation.sheet = .resetPins })
        }
    }

    private func banner(_ spec: BannerSpec) -> Banner {
        switch spec.action {
        case .editHost?:
            return Banner(
                tone: spec.tone, text: spec.text, detail: spec.detail, actionTitle: "Edit host",
                action: { model.navigation.sheet = .editHost(host.id) })
        case .retry?, nil:
            return Banner(
                tone: spec.tone, text: spec.text, detail: spec.detail, actionTitle: "Try again",
                action: { model.restart(hostId: host.id) })
        }
    }
}
