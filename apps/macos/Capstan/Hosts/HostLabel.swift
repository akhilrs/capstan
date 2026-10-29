import CapstanAppCore
import CapstanKit
import SwiftUI

/// Names and states of hosts and projects as the views show them. Host text is cleaned and cut before it is shown.
enum HostLabel {
    static let nameCap = 64

    /// Control, bidi and line characters become spaces; the text is cut to `maxBytes` between characters.
    static func plain(_ s: String, maxBytes: Int = nameCap) -> String {
        let text = UntrustedText(raw: s, verified: true, truncated: false).displayPlain
        return TextBudget.displayPrefix(text, maxBytes: maxBytes).text
    }

    static func hostName(_ config: HostConfig) -> String {
        let name = plain(config.displayName)
        return name.trimmingCharacters(in: .whitespaces).isEmpty ? plain(config.destination) : name
    }

    static func projectName(_ project: ProjectModel) -> String {
        plain(ProjectName.display(root: project.root))
    }

    /// The projects of a host in name order.
    static func projects(_ host: HostModel?) -> [ProjectModel] {
        (host?.projects.values).map { Array($0) }?.sorted {
            let a = projectName($0), b = projectName($1)
            return a == b ? $0.projectId < $1.projectId : a.localizedStandardCompare(b) == .orderedAscending
        } ?? []
    }

    /// The glyph of a connection state in a pill.
    static func symbol(_ state: ConnectionState?) -> String {
        switch state {
        case nil: return "circle.dashed"
        case .connecting?: return "arrow.triangle.2.circlepath"
        case .live?: return "dot.radiowaves.left.and.right"
        case .noHeartbeat?: return "heart.slash"
        case .reconnecting?: return "arrow.clockwise"
        case .stopped?: return "stop.circle"
        }
    }

    static func tone(_ availability: ProjectAvailability) -> Tone {
        switch availability {
        case .ready: return .green
        case .silent: return .yellow
        case .noKey, .refused, .unsupported, .gone, .other: return .red
        }
    }

    static func pill(_ state: ConnectionState?, loading: Bool = false) -> StatePill {
        let spec = CapsuleChoice.make(state: state, anyLoading: loading, heartbeatPulse: false, reduceMotion: true)
        return StatePill(word: spec.label, symbol: symbol(state), tone: spec.tone)
    }
}
