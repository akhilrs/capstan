import CapstanKit
import Foundation

/// A host as the voice features see it: its id, the name that is spoken, and what the phone knows of it.
public struct VoiceHostModel: Sendable, Equatable {
    public let id: UUID
    public let name: String
    public let model: HostModel

    public init(id: UUID, name: String, model: HostModel) {
        self.id = id
        self.name = name
        self.model = model
    }
}

/// A host that did not answer: its id and the name that is spoken.
public struct VoiceHostRef: Sendable, Equatable {
    public let id: UUID
    public let name: String

    public init(id: UUID, name: String) {
        self.id = id
        self.name = name
    }
}

/// Whether a text from a host may be read aloud.
public enum VoiceTextState: Sendable, Equatable {
    case readable
    case unverified
    /// Cut by the host, or over ITEM_CAP.
    case tooLong
    case multiline
    /// A control character, a bidi mark, a non-ASCII or look-alike letter.
    case nonAscii
}

/// The oldest eligible Operator proposal across hosts, with what the decision needs.
public struct VoiceTarget: Sendable, Equatable {
    public let hostId: UUID
    public let hostName: String
    public let projectId: String
    public let projectName: String
    public let key: String
    public let requestId: String
    /// The hash of the item as it was read back; the decision is sent with it.
    public let contentHash: String
    public let item: StoredWaiting
    /// `where.project` or `where.projectHost`, filled.
    public let whereText: String
    /// Older Operator proposals that were passed over because voice cannot decide them.
    public let skippedOlder: Int
}

/// One answer Siri may offer for the target.
public enum VoiceChoice: Sendable, Equatable {
    case once
    case exact
    /// `raw` is the prefix byte for byte as the host offered it; `number` counts from 1 in host order.
    case prefix(raw: String, number: Int)
    case autoHour
    case autoSession
    /// An old host (no `approveChoices`): the forwarded `op approve`.
    case forwardedApprove
    case deny
}

/// Why a command cannot be approved (or decided) by voice.
public enum VoiceRefusalKind: Sendable, Equatable {
    case voiceOff
    case tooLong
    /// `reasonKey` is a `reason.*` key of `strings.json`.
    case notDecidable(reasonKey: String)
    /// It can be denied but not approved; `reasonKey` is a `reason.*` key.
    case denyOnly(reasonKey: String)
    /// Not an Operator proposal.
    case notACommand
}

/// What voice may do with one item.
public struct VoiceChoices: Sendable, Equatable {
    /// The approve answers in the order the design numbers them; empty when none is allowed.
    public let approve: [VoiceChoice]
    public let canDeny: Bool
    public let refusal: VoiceRefusalKind?
    /// More was offered by the host, but Grants by voice is off.
    public let grantsOff: Bool
    /// Grants by voice is on, but the host offers only approve once.
    public let onlyOnce: Bool
    /// The host sends no `approveChoices`.
    public let oldHost: Bool

    public init(
        approve: [VoiceChoice], canDeny: Bool, refusal: VoiceRefusalKind? = nil, grantsOff: Bool = false,
        onlyOnce: Bool = false, oldHost: Bool = false
    ) {
        self.approve = approve
        self.canDeny = canDeny
        self.refusal = refusal
        self.grantsOff = grantsOff
        self.onlyOnce = onlyOnce
        self.oldHost = oldHost
    }

    /// Approve once: the narrowest yes, or the forwarded approve of an old host.
    public var approveOnce: VoiceChoice? {
        approve.first { $0 == .once || $0 == .forwardedApprove }
    }
}

/// The listing, the target and the choices of the voice features (plan-156 design-voice). Pure: no I/O.
public enum VoiceDesk {
    // MARK: Text

    /// Whether the text may be spoken: verified, whole, within ITEM_CAP and printable ASCII only.
    public static func textState(_ text: UntrustedText) -> VoiceTextState {
        guard text.verified, !text.raw.isEmpty else { return .unverified }
        if text.truncated || text.raw.count > VoiceTexts.ITEM_CAP { return .tooLong }
        return plainState(text.raw)
    }

    /// The state of a plain string such as a project name (the length is checked too).
    public static func plainState(_ raw: String) -> VoiceTextState {
        if raw.isEmpty { return .unverified }
        if raw.count > VoiceTexts.ITEM_CAP { return .tooLong }
        var multiline = false
        var other = false
        for scalar in raw.unicodeScalars {
            if scalar == "\n" || scalar == "\r" {
                multiline = true
            } else if scalar.value < 0x20 || scalar.value > 0x7E {
                other = true
            }
        }
        if other { return .nonAscii }
        return multiline ? .multiline : .readable
    }

    static func hasShellOperator(_ text: String) -> Bool {
        text.contains("|") || text.contains("&&") || text.contains(";") || text.contains(">")
    }

    static func parseTime(_ text: String) -> Date? {
        let plain = ISO8601DateFormatter()
        if let date = plain.date(from: text) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text)
    }

    /// "just now", "5 minutes ago", "1 hour ago", "2 days ago".
    public static func age(since: String, now: Date) -> String {
        guard let date = parseTime(since) else { return VoiceTexts.text("age.now") }
        let seconds = max(0, now.timeIntervalSince(date))
        let minutes = Int(seconds / 60)
        if minutes < 1 { return VoiceTexts.text("age.now") }
        if minutes < 60 {
            return minutes == 1 ? VoiceTexts.text("age.minute") : VoiceTexts.fill("age.minutes", ["n": String(minutes)])
        }
        let hours = minutes / 60
        if hours < 24 {
            return hours == 1 ? VoiceTexts.text("age.hour") : VoiceTexts.fill("age.hours", ["n": String(hours)])
        }
        let days = hours / 24
        return days == 1 ? VoiceTexts.text("age.day") : VoiceTexts.fill("age.days", ["n": String(days)])
    }

    // MARK: Entries

    struct Entry {
        let host: VoiceHostModel
        let projectId: String
        let projectName: String
        let item: StoredWaiting
        let date: Date?

        var projectReadable: Bool { VoiceDesk.plainState(projectName) == .readable }

        var eligible: Bool {
            item.kind == .op && !item.needsTerminal && VoiceDesk.textState(item.summary) == .readable
                && projectReadable
        }
    }

    static func entries(_ models: [VoiceHostModel]) -> [Entry] {
        var all: [Entry] = []
        for host in models {
            for (projectId, project) in host.model.projects {
                for item in project.waiting.values {
                    all.append(Entry(
                        host: host, projectId: projectId, projectName: project.displayName, item: item,
                        date: parseTime(item.since)))
                }
            }
        }
        return all.sorted(by: before)
    }

    /// Oldest first; ties by host id, project id and key, ascending.
    static func before(_ a: Entry, _ b: Entry) -> Bool {
        switch (a.date, b.date) {
        case (let x?, let y?) where x != y: return x < y
        case (nil, .some): return false
        case (.some, nil): return true
        case (nil, nil) where a.item.since != b.item.since: return a.item.since < b.item.since
        default: break
        }
        if a.host.id != b.host.id { return a.host.id.uuidString < b.host.id.uuidString }
        if a.projectId != b.projectId { return a.projectId < b.projectId }
        return a.item.key < b.item.key
    }

    /// The project as it is spoken: its name, with the host when another host has a project of the same name.
    static func whereText(_ entry: Entry, in models: [VoiceHostModel]) -> String {
        guard entry.projectReadable else { return VoiceTexts.text("where.unreadable") }
        let clash = models.contains { other in
            other.id != entry.host.id && other.model.projects.values.contains { $0.displayName == entry.projectName }
        }
        guard clash else { return VoiceTexts.fill("where.project", ["project": entry.projectName]) }
        return VoiceTexts.fill("where.projectHost", ["project": entry.projectName, "host": entry.host.name])
    }

    // MARK: Listing

    /// What waits, oldest first, at most three items. `decisions` is Decide by voice: the hint is spoken only then.
    public static func listing(
        _ models: [VoiceHostModel], unreachable: [VoiceHostRef], now: Date, decisions: Bool = false
    ) -> [String] {
        let down = Set(unreachable.map(\.id))
        let live = models.filter { !down.contains($0.id) }
        let all = entries(live)
        var lines: [String] = []
        if all.isEmpty && !unreachable.isEmpty && live.isEmpty {
            return [VoiceTexts.text("list.unreachable.all")]
        }
        switch all.count {
        case 0: lines.append(VoiceTexts.text("list.count.none"))
        case 1: lines.append(VoiceTexts.text("list.count.one"))
        default: lines.append(VoiceTexts.fill("list.count.many", ["n": String(all.count)]))
        }
        for entry in all.prefix(3) { lines.append(itemLine(entry, live, now)) }
        let more = all.count - 3
        if more == 1 {
            lines.append(VoiceTexts.text("list.more.one"))
        } else if more > 1 {
            lines.append(VoiceTexts.fill("list.more.many", ["n": String(more)]))
        }
        if decisions && all.contains(where: { $0.eligible }) { lines.append(VoiceTexts.text("list.hint")) }
        for host in unreachable { lines.append(VoiceTexts.fill("list.unreachable", ["host": host.name])) }
        return lines
    }

    private static func itemLine(_ entry: Entry, _ models: [VoiceHostModel], _ now: Date) -> String {
        let values = [
            "where": whereText(entry, in: models), "age": age(since: entry.item.since, now: now),
            "kind": VoiceTexts.text(kindKey(entry.item.kind)),
        ]
        guard entry.item.kind == .op else { return VoiceTexts.fill("list.item.plain", values) }
        switch textState(entry.item.summary) {
        case .unverified: return VoiceTexts.fill("list.item.unverified", values)
        case .tooLong: return VoiceTexts.fill("list.item.long", values)
        case .multiline, .nonAscii: return VoiceTexts.fill("list.item.unreadable", values)
        case .readable:
            return entry.projectReadable
                ? VoiceTexts.fill("list.item.plain", values) : VoiceTexts.fill("list.item.unreadable", values)
        }
    }

    static func kindKey(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "kind.op"
        case .prompt: return "kind.prompt"
        case .design: return "kind.design"
        case .message: return "kind.message"
        case .integration: return "kind.integration"
        case .other: return "kind.other"
        }
    }

    // MARK: Target

    /// The oldest eligible Operator proposal across hosts; nil when none is eligible.
    public static func target(_ models: [VoiceHostModel]) -> VoiceTarget? {
        let all = entries(models)
        guard let index = all.firstIndex(where: { $0.eligible }) else { return nil }
        let entry = all[index]
        let skipped = all[..<index].filter { $0.item.kind == .op }.count
        return VoiceTarget(
            hostId: entry.host.id, hostName: entry.host.name, projectId: entry.projectId,
            projectName: entry.projectName, key: entry.item.key, requestId: entry.item.requestId,
            contentHash: entry.item.contentHash, item: entry.item, whereText: whereText(entry, in: models),
            skippedOlder: skipped)
    }

    /// The line to speak when `target` found nothing: nothing waits, only other kinds wait, or the oldest command
    /// cannot be decided by voice.
    public static func noTarget(_ models: [VoiceHostModel]) -> String {
        let all = entries(models)
        guard let oldest = all.first(where: { $0.item.kind == .op }) else {
            switch all.count {
            case 0: return VoiceTexts.text("refusal.nothing")
            case 1: return VoiceTexts.text("refusal.readOnly.one")
            default: return VoiceTexts.fill("refusal.readOnly.many", ["n": String(all.count)])
            }
        }
        let place = whereText(oldest, in: models)
        if oldest.item.needsTerminal { return VoiceTexts.text("refusal.needsTerminal") }
        switch textState(oldest.item.summary) {
        case .tooLong: return VoiceTexts.fill("refusal.tooLong", ["project": place])
        case .unverified: return notDecidable(place, "reason.unverified")
        case .multiline: return notDecidable(place, "reason.multiline")
        case .nonAscii: return notDecidable(place, "reason.nonAscii")
        case .readable: return notDecidable(place, "reason.project")
        }
    }

    private static func notDecidable(_ place: String, _ reasonKey: String) -> String {
        VoiceTexts.fill("refusal.notDecidable", ["project": place, "reason": VoiceTexts.text(reasonKey)])
    }

    /// The spoken line of a refusal of the choices.
    public static func refusalLine(_ kind: VoiceRefusalKind, target: VoiceTarget?) -> String {
        let place = target?.whereText ?? VoiceTexts.text("where.unreadable")
        switch kind {
        case .voiceOff: return VoiceTexts.text("refusal.voiceOff")
        case .tooLong: return VoiceTexts.fill("refusal.tooLong", ["project": place])
        case .notDecidable(let key): return notDecidable(place, key)
        case .denyOnly(let key):
            return VoiceTexts.fill("refusal.denyOnly", ["project": place, "reason": VoiceTexts.text(key)])
        case .notACommand: return VoiceTexts.text("refusal.nothing")
        }
    }

    // MARK: Choices

    /// The answers voice allows for an Operator proposal, under the two switches.
    public static func choices(_ item: StoredWaiting, _ settings: PhoneSettings) -> VoiceChoices {
        guard settings.voiceDecisions else { return VoiceChoices(approve: [], canDeny: false, refusal: .voiceOff) }
        guard item.kind == .op else { return VoiceChoices(approve: [], canDeny: false, refusal: .notACommand) }
        switch textState(item.summary) {
        case .readable: break
        case .tooLong: return VoiceChoices(approve: [], canDeny: false, refusal: .tooLong)
        case .unverified:
            return VoiceChoices(approve: [], canDeny: false, refusal: .notDecidable(reasonKey: "reason.unverified"))
        case .multiline:
            return VoiceChoices(approve: [], canDeny: false, refusal: .notDecidable(reasonKey: "reason.multiline"))
        case .nonAscii:
            return VoiceChoices(approve: [], canDeny: false, refusal: .notDecidable(reasonKey: "reason.nonAscii"))
        }
        func denyOnly(_ key: String, oldHost: Bool = false) -> VoiceChoices {
            VoiceChoices(approve: [], canDeny: true, refusal: .denyOnly(reasonKey: key), oldHost: oldHost)
        }
        let old = item.approveChoices == nil
        if let offered = item.approveChoices, let limit = offered.limit {
            switch limit {
            case .floor: return denyOnly("reason.floor")
            case .restart: return denyOnly("reason.restart")
            case .notChecked: return denyOnly("reason.not_checked")
            case .textMismatch: return denyOnly("reason.text_mismatch")
            case .truncated: return VoiceChoices(approve: [], canDeny: false, refusal: .tooLong)
            case .other: return denyOnly("reason.not_checked")
            }
        }
        if hasShellOperator(item.summary.raw) { return denyOnly("reason.shell", oldHost: old) }
        guard let offered = item.approveChoices else {
            return VoiceChoices(approve: [.forwardedApprove], canDeny: true, oldHost: true)
        }
        var approve: [VoiceChoice] = []
        if offered.once { approve.append(.once) }
        if approve.isEmpty { return denyOnly("reason.not_checked") }
        var grantable: [VoiceChoice] = []
        if offered.exact { grantable.append(.exact) }
        var number = 0
        for prefix in offered.prefixes.prefix(3) where textState(prefix) == .readable {
            number += 1
            grantable.append(.prefix(raw: prefix.raw, number: number))
        }
        if offered.auto.contains(.hour) { grantable.append(.autoHour) }
        if offered.auto.contains(.session) { grantable.append(.autoSession) }
        let grantsOff = !settings.voiceGrants && !grantable.isEmpty
        if settings.voiceGrants { approve += grantable }
        let onlyOnce = settings.voiceGrants && grantable.isEmpty
        return VoiceChoices(approve: approve, canDeny: true, grantsOff: grantsOff, onlyOnce: onlyOnce)
    }

    // MARK: Spoken results

    /// What Siri says after a decision. `sent` is the answer that was sent; the host's `approval` wins when it names
    /// a different choice.
    public static func spoken(_ outcome: DecideOutcome, sent: VoiceChoice, target: VoiceTarget) -> [String] {
        let project = target.projectName
        let host = target.hostName
        switch outcome {
        case .applied(let result):
            return appliedLines(result, sent: sent, project: project)
        case .forwarded:
            return [VoiceTexts.fill("result.forwarded", ["project": project])]
        case .refused(let refusal, _):
            return [refusalText(refusal.code, project: project, host: host)]
        case .notSent(let reason, _):
            switch reason {
            case .couldNotStart, .ssh: return [VoiceTexts.fill("refusal.unreachable", ["host": host])]
            case .timeout: return [VoiceTexts.fill("result.unknown", ["host": host])]
            }
        case .malformed:
            return [VoiceTexts.fill("result.unknown", ["host": host])]
        }
    }

    static func refusalText(_ code: DecideErrorCode, project: String, host: String) -> String {
        let values = ["project": project, "host": host]
        switch code {
        case .hashMismatch, .notWaiting: return VoiceTexts.text("refusal.changed")
        case .needsTerminal: return VoiceTexts.text("refusal.needsTerminal")
        case .noActivePm: return VoiceTexts.fill("refusal.noActivePm", values)
        case .limitReached: return VoiceTexts.text("refusal.limitReached")
        case .other(let name) where name == "busy": return VoiceTexts.fill("refusal.busy", values)
        default: return VoiceTexts.fill("refusal.other", values)
        }
    }

    private static func appliedLines(_ result: DecideResult, sent: VoiceChoice, project: String) -> [String] {
        let values = ["project": project]
        if result.approval == nil && (result.verb == "deny" || sent == .deny) {
            return [VoiceTexts.fill("result.denied", values)]
        }
        let word = result.approval?.choice ?? choiceWord(sent)
        let time = result.approval?.expiresAt.flatMap { plainState($0) == .readable ? $0 : nil }
        func timed(_ key: String) -> String {
            guard let time else { return VoiceTexts.fill(key + ".noEnd", values) }
            return VoiceTexts.fill(key, ["project": project, "time": time])
        }
        switch word {
        case "exact": return [timed("result.exact")]
        case "prefix": return [timed("result.prefix")]
        case "auto-hour", "auto-session":
            var lines = [timed("result.window")]
            if let id = result.approval?.windowId, plainState(id) == .readable {
                lines.append(VoiceTexts.fill("result.window.revoke", ["windowId": id]))
            }
            return lines
        default: return [VoiceTexts.fill("result.approved", values)]
        }
    }

    private static func choiceWord(_ choice: VoiceChoice) -> String {
        switch choice {
        case .once, .forwardedApprove, .deny: return "once"
        case .exact: return "exact"
        case .prefix: return "prefix"
        case .autoHour: return "auto-hour"
        case .autoSession: return "auto-session"
        }
    }
}
