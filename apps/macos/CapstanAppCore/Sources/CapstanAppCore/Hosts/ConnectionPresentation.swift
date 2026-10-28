import CapstanKit
import Foundation

public struct CapsuleSpec: Sendable, Equatable {
    public let label: String
    public let tone: Tone
    public let pulse: Bool

    public init(label: String, tone: Tone, pulse: Bool) {
        self.label = label
        self.tone = tone
        self.pulse = pulse
    }
}

public enum CapsuleChoice {
    public static func make(
        state: ConnectionState?, anyLoading: Bool, heartbeatPulse: Bool, reduceMotion: Bool
    ) -> CapsuleSpec {
        guard let state else { return CapsuleSpec(label: "Not connected", tone: .grey, pulse: false) }
        switch state {
        case .connecting:
            return CapsuleSpec(label: "Connecting", tone: .yellow, pulse: false)
        case .live:
            if anyLoading { return CapsuleSpec(label: "Loading", tone: .blue, pulse: false) }
            return CapsuleSpec(label: "Live", tone: .green, pulse: heartbeatPulse && !reduceMotion)
        case .noHeartbeat:
            return CapsuleSpec(label: "No heartbeat", tone: .yellow, pulse: false)
        case .reconnecting:
            return CapsuleSpec(label: "Reconnecting", tone: .yellow, pulse: false)
        case .stopped(let reason):
            if case .user = reason { return CapsuleSpec(label: "Stopped", tone: .grey, pulse: false) }
            return CapsuleSpec(label: "Stopped", tone: .red, pulse: false)
        }
    }
}

public enum BannerKind: Sendable, Equatable {
    case state, loading, noDecide
}

public enum BannerAction: Sendable, Equatable {
    case retry, editHost
}

public enum BannerActionResolution: Sendable, Equatable {
    case restart, openEditor, none
}

public struct BannerSpec: Sendable, Equatable {
    public let kind: BannerKind
    public let tone: Tone
    public let text: String
    public let detail: UntrustedText?
    public let action: BannerAction?
    public let retryAt: Date?

    public init(
        kind: BannerKind, tone: Tone, text: String, detail: UntrustedText? = nil,
        action: BannerAction? = nil, retryAt: Date? = nil
    ) {
        self.kind = kind
        self.tone = tone
        self.text = text
        self.detail = detail
        self.action = action
        self.retryAt = retryAt
    }
}

public enum BannerChoice {
    public static func make(
        state: ConnectionState?, anyLoading: Bool, decide: Bool?, hostName: String, destination: String,
        now: Date
    ) -> [BannerSpec] {
        var banners: [BannerSpec] = []
        let name = HostText.name(hostName)
        if let state, let banner = stateBanner(state, name: name, destination: destination, now: now) {
            banners.append(banner)
        }
        if anyLoading, case .live = state {
            banners.append(BannerSpec(kind: .loading, tone: .blue, text: "Loading projects"))
        }
        if decide == false {
            banners.append(
                BannerSpec(
                    kind: .noDecide, tone: .grey,
                    text: "This host's Capstan has no remote decisions; answers need the terminal"))
        }
        return banners
    }

    /// What a banner button does, given what the window can do now.
    public static func resolveAction(
        _ a: BannerAction, configExists: Bool, sheetPresented: Bool
    ) -> BannerActionResolution {
        switch a {
        case .retry: return .restart
        case .editHost: return configExists && !sheetPresented ? .openEditor : .none
        }
    }

    private static func stateBanner(
        _ state: ConnectionState, name: String, destination: String, now: Date
    ) -> BannerSpec? {
        switch state {
        case .connecting, .live:
            return nil
        case .noHeartbeat(let since):
            let seconds = max(0, Int(now.timeIntervalSince(since).rounded(.down)))
            return BannerSpec(kind: .state, tone: .yellow, text: "No heartbeat for \(seconds) s")
        case .reconnecting(_, let retryAt, let error):
            guard let error, let failure = failure(error, name: name, destination: destination) else {
                return nil
            }
            return BannerSpec(
                kind: .state, tone: .yellow, text: failure.text, detail: failure.detail, retryAt: retryAt)
        case .stopped(let reason):
            return stopBanner(reason, name: name)
        }
    }

    private static func failure(
        _ error: SessionError, name: String, destination: String
    ) -> (text: String, detail: UntrustedText?)? {
        switch error {
        case .interrupted:
            return nil
        case .ssh(let tail):
            var text = "Could not reach \(name) over SSH"
            let detail = HostText.detail(tail)
            if let hint = HostText.sshHint(detail?.raw ?? "", destination: destination) { text += ". " + hint }
            return (text, detail)
        case .stream(let code, _):
            return ("Capstan on \(name) stopped the stream: \(HostText.code(code))", nil)
        case .exit(let n, _):
            switch n {
            case 0: return nil
            case 1: return ("The stream ended with an error (exit 1)", nil)
            case 129...: return ("The stream ended with exit \(n) (signal \(n - 128))", nil)
            default: return ("The stream ended with exit \(n)", nil)
            }
        }
    }

    private static func stopBanner(_ reason: StopReason, name: String) -> BannerSpec? {
        switch reason {
        case .user:
            return nil
        case .unsupportedVersion:
            return BannerSpec(kind: .state, tone: .red, text: "Update this app")
        case .hostRejected(let code, let tail):
            let shown = code.map { HostText.code($0) } ?? "no code"
            return BannerSpec(
                kind: .state, tone: .red, text: "The host refused: \(shown)", detail: HostText.detail(tail))
        case .cstanMissing(let tail):
            return BannerSpec(
                kind: .state, tone: .red, text: "cstan was not found on \(name); set the cstan path",
                detail: HostText.detail(tail), action: .editHost)
        case .repeatedFailure:
            return BannerSpec(kind: .state, tone: .red, text: "The stream failed 5 times", action: .retry)
        }
    }
}

/// "Retrying in N s" with N the remaining seconds rounded up; "Retrying now" when none are left.
public func countdown(retryAt: Date, now: Date) -> String {
    let remaining = retryAt.timeIntervalSince(now)
    if remaining <= 0 { return "Retrying now" }
    return "Retrying in \(Int(remaining.rounded(.up))) s"
}

public struct CheckStepText: Sendable, Equatable {
    public let title: String
    public let line: String

    public init(title: String, line: String) {
        self.title = title
        self.line = line
    }

    public static func make(_ check: HostCheck, destination: String) -> CheckStepText {
        let title: String
        switch check.step {
        case .connect: title = "Connect over SSH"
        case .capstan: title = "Find Capstan on the host"
        case .projects: title = "List projects"
        case .firstSnapshot: title = "First snapshot"
        }
        let status: String
        switch check.status {
        case .passed: status = "Passed"
        case .warning: status = "Warning"
        case .failed: status = "Failed"
        }
        let text = HostText.clean(check.text, maxBytes: HostText.detailCap)
        var line = text.isEmpty ? status : text
        if check.status == .failed, let hint = HostText.sshHint(text, destination: destination) {
            line += " " + hint
        }
        return CheckStepText(title: title, line: line)
    }
}
