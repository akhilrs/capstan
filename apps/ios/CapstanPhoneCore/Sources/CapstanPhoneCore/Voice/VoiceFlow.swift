import CapstanKit
import Foundation

/// What the app can do right now, computed from `KeychainKeyVault.current()`: a throw is `keysUnreadable`, nil is
/// `notPaired`.
public enum VoiceReadiness: Sendable, Equatable {
    case ready
    case keysUnreadable
    case notPaired
    case noHosts
}

public enum VoiceAction: Sendable, Equatable {
    case list
    case approveOnce
    /// Approve with a choice: Siri offers what the host and the settings allow.
    case approve
    case deny
}

/// What the hosts held when they were read.
public struct VoiceRead: Sendable {
    public let models: [VoiceHostModel]
    public let unreachable: [VoiceHostRef]
    public let now: Date

    public init(models: [VoiceHostModel], unreachable: [VoiceHostRef] = [], now: Date) {
        self.models = models
        self.unreachable = unreachable
        self.now = now
    }
}

/// A question Siri asks that is answered by a number.
public struct VoicePrompt: Sendable, Equatable {
    public let question: String
    /// One spoken line per option; the answer is the index (from 0) of the option chosen.
    public let options: [String]

    public init(question: String, options: [String]) {
        self.question = question
        self.options = options
    }
}

/// Why the hosts could not be read.
public enum VoiceEnvironmentError: Error, Sendable, Equatable {
    case timeout(host: String)
    case unreachable(host: String)
}

/// What the intents provide. The app implements it; tests use a fake.
public protocol VoiceEnvironment: Sendable {
    /// Read first. Anything but `ready` is spoken and nothing else happens.
    func readiness() async -> VoiceReadiness
    func settings() async -> PhoneSettings
    func nonce() -> String
    /// Reads the hosts within `budget` seconds and holds a `SessionLease` while `body` runs. Everything that talks to
    /// a host, the confirmation and the send included, happens inside `body`.
    func withHosts<T: Sendable>(budget: TimeInterval, _ body: @Sendable (VoiceRead) async throws -> T) async throws -> T
    /// Asks a yes/no question aloud; false for a no or a cancel.
    func confirm(_ text: String) async -> Bool
    /// Asks for a number; nil when none is heard or the person cancels.
    func choose(_ prompt: VoicePrompt) async -> Int?
    func voiceSend(_ request: DecideRequest) async -> DecideOutcome
    var isBackground: Bool { get }
}

/// The whole intent flow, so its order is tested here: readiness, then (inside the hosts read) target, choices,
/// the read-back with its confirmation, and one send.
public enum VoiceFlow {
    /// The longest the hosts may take to answer, in seconds (design: 8 s budget).
    public static let readBudget: TimeInterval = 8

    public static func run(_ action: VoiceAction, _ env: any VoiceEnvironment) async -> [String] {
        switch await env.readiness() {
        case .ready: break
        case .keysUnreadable: return [VoiceTexts.text("refusal.locked")]
        case .notPaired, .noHosts: return [VoiceTexts.text("refusal.notPaired")]
        }
        let settings = await env.settings()
        if action != .list && !settings.voiceDecisions { return [VoiceTexts.text("refusal.voiceOff")] }
        do {
            return try await env.withHosts(budget: readBudget) { read in
                await decide(action, read, settings, env)
            }
        } catch VoiceEnvironmentError.timeout(let host) {
            return [VoiceTexts.fill("refusal.timeout", ["host": host])]
        } catch VoiceEnvironmentError.unreachable(let host) {
            return [VoiceTexts.fill("refusal.unreachable", ["host": host])]
        } catch {
            return [VoiceTexts.fill("refusal.unreachable", ["host": ""])]
        }
    }

    private static func decide(
        _ action: VoiceAction, _ read: VoiceRead, _ settings: PhoneSettings, _ env: any VoiceEnvironment
    ) async -> [String] {
        if action == .list {
            return VoiceDesk.listing(
                read.models, unreachable: read.unreachable, now: read.now, decisions: settings.voiceDecisions)
        }
        guard let target = VoiceDesk.target(read.models) else { return [VoiceDesk.noTarget(read.models)] }
        let choices = VoiceDesk.choices(target.item, settings)
        var lines: [String] = []
        let choice: VoiceChoice
        var chosen: [String] = []
        switch action {
        case .deny:
            guard choices.canDeny else { return refusal(choices, target) }
            choice = .deny
        case .approveOnce, .approve:
            guard let once = choices.approveOnce else { return refusal(choices, target) }
            if action == .approveOnce {
                choice = once
            } else {
                if choices.oldHost {
                    lines.append(VoiceTexts.fill("choose.oldHost", ["host": target.hostName]))
                    choice = once
                } else if choices.grantsOff {
                    lines.append(VoiceTexts.text("choose.grantsOff"))
                    choice = once
                } else if choices.onlyOnce {
                    lines.append(VoiceTexts.text("choose.onlyOnce"))
                    choice = once
                } else {
                    switch await pick(choices, env) {
                    case .some(let picked): choice = picked
                    case .none: return [VoiceTexts.text("choose.unclear")]
                    }
                }
            }
        case .list:
            return []
        }
        chosen = readBack(choice, target)
        let readBackText = chosen.joined(separator: " ")
        guard await env.confirm(readBackText) else { return lines + [VoiceTexts.text("confirm.cancelled")] }
        if let second = secondYes(choice, target) {
            guard await env.confirm(second) else { return lines + [VoiceTexts.text("confirm.cancelled")] }
        }
        let request: DecideRequest
        do {
            request = try VoiceRequest.make(choice: choice, target: target, settings: settings, nonce: env.nonce())
        } catch {
            return lines + refusal(choices, target)
        }
        let outcome = await env.voiceSend(request)
        return lines + VoiceDesk.spoken(outcome, sent: choice, target: target)
    }

    private static func refusal(_ choices: VoiceChoices, _ target: VoiceTarget) -> [String] {
        guard let kind = choices.refusal else { return [VoiceTexts.text("refusal.nothing")] }
        return [VoiceDesk.refusalLine(kind, target: target)]
    }

    /// Asks which way to approve; a prefix asks a second question when more than one is offered.
    private static func pick(_ choices: VoiceChoices, _ env: any VoiceEnvironment) async -> VoiceChoice? {
        let offered = choices.approve
        let prefixes = offered.filter { if case .prefix = $0 { return true } else { return false } }
        var groups: [VoiceChoice] = []
        for choice in offered {
            if case .prefix = choice {
                if prefixes.first == choice { groups.append(choice) }
            } else {
                groups.append(choice)
            }
        }
        var options: [String] = []
        for (index, choice) in groups.enumerated() {
            options.append(VoiceTexts.fill("prompt.choice.option", [
                "number": String(index + 1), "label": VoiceTexts.text(labelKey(choice)),
            ]))
        }
        let question = VoicePrompt(question: VoiceTexts.text("prompt.choice"), options: options)
        guard let index = await env.choose(question), groups.indices.contains(index) else { return nil }
        let picked = groups[index]
        guard case .prefix = picked, prefixes.count > 1 else { return picked }
        var lines: [String] = []
        for choice in prefixes {
            if case .prefix(let raw, let number) = choice {
                lines.append(VoiceTexts.fill("prompt.prefix.option", ["number": String(number), "prefix": raw]))
            }
        }
        let second = VoicePrompt(question: VoiceTexts.text("prompt.prefix"), options: lines)
        guard let which = await env.choose(second), prefixes.indices.contains(which) else { return nil }
        return prefixes[which]
    }

    private static func labelKey(_ choice: VoiceChoice) -> String {
        switch choice {
        case .once, .forwardedApprove, .deny: return "choose.label.once"
        case .exact: return "choose.label.exact"
        case .prefix: return "choose.label.prefix"
        case .autoHour: return "choose.label.hour"
        case .autoSession: return "choose.label.session"
        }
    }

    /// The read-back: the project, the command inside its frame, what the answer grants, then the fixed question.
    static func readBack(_ choice: VoiceChoice, _ target: VoiceTarget) -> [String] {
        var lines: [String] = []
        let place = ["where": target.whereText]
        switch target.skippedOlder {
        case 0: break
        case 1: lines.append(VoiceTexts.text("readback.skipped.one"))
        default: lines.append(VoiceTexts.fill("readback.skipped.many", ["n": String(target.skippedOlder)]))
        }
        switch choice {
        case .deny: lines.append(VoiceTexts.fill("readback.deny", place))
        case .once, .forwardedApprove: lines.append(VoiceTexts.fill("readback.approve", place))
        default: lines.append(VoiceTexts.fill("readback.approveAnd", place))
        }
        lines.append(VoiceTexts.fill("frame.command", ["command": target.item.summary.raw]))
        switch choice {
        case .deny, .once, .forwardedApprove:
            break
        case .exact:
            lines.append(VoiceTexts.text("readback.exact"))
        case .prefix(let raw, let number):
            lines.append(VoiceTexts.fill("readback.prefix", ["number": String(number), "prefix": raw]))
            lines.append(VoiceTexts.text("readback.prefix.rule"))
            lines.append(VoiceTexts.text("readback.prefix.length"))
        case .autoHour, .autoSession:
            lines.append(VoiceTexts.text(choice == .autoHour ? "readback.hour" : "readback.session"))
            for key in ["all", "floor", "scripts", "tools"] { lines.append(VoiceTexts.text("readback.coverage." + key)) }
        }
        switch choice {
        case .deny: lines.append(VoiceTexts.text("confirm.deny"))
        case .once, .forwardedApprove: lines.append(VoiceTexts.text("confirm.approve"))
        case .exact, .prefix: lines.append(VoiceTexts.text("confirm.approveAnd"))
        case .autoHour, .autoSession: lines.append(VoiceTexts.text("confirm.window"))
        }
        return lines
    }

    /// An auto window asks a second time after its coverage was read.
    static func secondYes(_ choice: VoiceChoice, _ target: VoiceTarget) -> String? {
        let values = ["project": target.projectName]
        switch choice {
        case .autoHour: return VoiceTexts.fill("prompt.secondYes.hour", values)
        case .autoSession: return VoiceTexts.fill("prompt.secondYes.session", values)
        default: return nil
        }
    }
}
