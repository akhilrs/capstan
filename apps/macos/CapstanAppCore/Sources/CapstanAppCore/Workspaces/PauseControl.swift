import CapstanKit
import Foundation

public enum ControlState: Sendable, Equatable {
    case enabled
    case disabled(String)

    public var isEnabled: Bool { self == .enabled }
}

public enum PauseReason {
    public static let noDecide = "This host's Capstan has no remote decisions"
    public static let sending = "Sending"
    public static let notPaused = "Not paused"
    public static let alreadyPaused = "Already paused"

    /// The reason a decision cannot be sent now; nil when it can.
    static func blocked(decide: Bool, busy: Bool) -> String? {
        if !decide { return noDecide }
        if busy { return sending }
        return nil
    }
}

/// The Pause and Resume buttons of one agent.
public struct PauseControl: Sendable, Equatable {
    public let pause: ControlState
    public let resume: ControlState

    /// `decide` is the host's capability; `busy` is whether a decision for this agent is in flight.
    public init(decide: Bool, busy: Bool, paused: Bool) {
        if let reason = PauseReason.blocked(decide: decide, busy: busy) {
            pause = .disabled(reason)
            resume = .disabled(reason)
        } else {
            pause = paused ? .disabled(PauseReason.alreadyPaused) : .enabled
            resume = paused ? .enabled : .disabled(PauseReason.notPaused)
        }
    }

    public init(capabilities: HostCapabilities?, busy: Set<BusyKey>, key: BusyKey, paused: Bool) {
        self.init(decide: capabilities?.decide ?? false, busy: busy.contains(key), paused: paused)
    }
}

/// The one button that pauses or resumes the whole project.
public struct ProjectPauseControl: Sendable, Equatable {
    public enum Action: Sendable, Equatable {
        case pause, resume
    }

    public static let pausedWord = "Project paused"

    public let action: Action
    public let state: ControlState
    /// Shown beside the button while the run is paused.
    public let status: String?

    public init(decide: Bool, busy: Bool, runPaused: Bool) {
        action = runPaused ? .resume : .pause
        state = PauseReason.blocked(decide: decide, busy: busy).map { .disabled($0) } ?? .enabled
        status = runPaused ? ProjectPauseControl.pausedWord : nil
    }

    public init(capabilities: HostCapabilities?, busy: Set<BusyKey>, key: BusyKey, runPaused: Bool) {
        self.init(decide: capabilities?.decide ?? false, busy: busy.contains(key), runPaused: runPaused)
    }
}
