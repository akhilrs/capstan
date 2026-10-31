import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// The words of a host's connection (boards 12 and 13): a short word and a time for the section header, and a title
/// with one sentence for the host's details. A problem the transport found wins over the stream's state.
enum HostStatusText {
    struct Header: Equatable {
        let word: String
        let trailing: String?
        let tone: Tone
    }

    struct Line: Equatable {
        let title: String
        let detail: String
        let tone: Tone
    }

    static func header(_ state: ConnectionState?, problem: HostProblem?, now: Date) -> Header {
        if problem != nil { return Header(word: "blocked", trailing: nil, tone: .red) }
        switch state {
        case .live(_, let lastLineAt)?:
            return Header(word: "live", trailing: seconds(since: lastLineAt, now: now) + " ago", tone: .green)
        case .connecting?:
            return Header(word: "connecting", trailing: nil, tone: .blue)
        case .noHeartbeat(let since)?:
            return Header(word: "no heartbeat", trailing: seconds(since: since, now: now), tone: .yellow)
        case .reconnecting(_, let retryAt, _)?:
            return Header(word: "reconnecting", trailing: retry(retryAt, now: now), tone: .yellow)
        case .stopped(.user)?:
            return Header(word: "not following", trailing: nil, tone: .grey)
        case .stopped?:
            return Header(word: "stopped", trailing: nil, tone: .red)
        case nil:
            return Header(word: "not connected", trailing: nil, tone: .grey)
        }
    }

    static func line(_ state: ConnectionState?, name: String, problem: HostProblem?, now: Date) -> Line {
        if problem != nil {
            return Line(title: "Blocked", detail: "The connection waits for the fix shown above.", tone: .red)
        }
        switch state {
        case .connecting?:
            return Line(title: "Connecting to " + name, detail: "SSH is starting cstan events.", tone: .blue)
        case .live(let since, let lastLineAt)?:
            return Line(
                title: name + " live · heartbeat " + seconds(since: lastLineAt, now: now) + " ago",
                detail: "Following since " + since.formatted(date: .omitted, time: .shortened) + ".", tone: .green)
        case .noHeartbeat(let since)?:
            return Line(
                title: name + " · no heartbeat for " + seconds(since: since, now: now),
                detail: "Three poll intervals passed with no line.", tone: .yellow)
        case .reconnecting(let attempt, let retryAt, _)?:
            return Line(
                title: "Reconnecting " + retry(retryAt, now: now) + " · attempt " + String(attempt),
                detail: "The stream ended. Retries wait 1, 2, 4, 8 … 60 s.", tone: .yellow)
        case .stopped(.user)?:
            return Line(
                title: "Not following", detail: "The app stopped following, or it was in the background.", tone: .grey)
        case .stopped(.unsupportedVersion(let version))?:
            return Line(
                title: "Not following", detail: "This host speaks protocol v" + String(version) + ".", tone: .red)
        case .stopped?:
            return Line(title: "Could not connect", detail: "The stream stopped; the reason is shown above.", tone: .red)
        case nil:
            return Line(title: "Not connected", detail: "The app has not started following this host yet.", tone: .grey)
        }
    }

    static func seconds(since: Date, now: Date) -> String {
        String(max(0, Int(now.timeIntervalSince(since)))) + " s"
    }

    static func retry(_ at: Date, now: Date) -> String {
        let left = Int(at.timeIntervalSince(now).rounded(.up))
        return left <= 0 ? "now" : "in " + String(left) + " s"
    }

    /// The colour of a state dot; never alone, the word is always beside it.
    static func color(_ tone: Tone) -> Color {
        tone == .grey || tone == .neutral ? Tokens.color(.label2) : tone.color
    }
}

/// A project state as the lists show it: the shared word, glyph, meaning and fix, with a tone.
extension AvailabilityText {
    static func tone(_ availability: ProjectAvailability) -> Tone {
        switch availability {
        case .ready: return .green
        case .noKey, .silent: return .yellow
        case .refused: return .red
        case .unsupported, .gone, .other: return .grey
        }
    }
}
