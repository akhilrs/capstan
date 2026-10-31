import Foundation

/// The scene phases the planner reads (SwiftUI's `ScenePhase`, without importing SwiftUI).
public enum PhonePhase: Sendable, Equatable {
    case active, inactive, background
}

/// What the app does on a phase change. The app runs them in order.
public enum LifecycleAction: Sendable, Equatable {
    /// Cover the content until Face ID passes.
    case lockUI
    case promptFaceID
    /// Remove the cover: the person unlocked, or no unlock was needed.
    case unlockUI
    case startSessions
    /// Stop every host session, then `TransportControl.suspend()`, in that order.
    case stopSessionsThenSuspend
    case noop
}

/// What the planner remembers between phase changes (plan section 1.2).
public struct LifecycleState: Sendable, Equatable {
    /// The last passed Face ID in this process; nil until one passed.
    public var lastUnlock: Date?
    /// When the scene last went to the background, cleared by an active check that passed or was not required.
    public var backgroundedAt: Date?
    /// The content is covered until an unlock.
    public var locked: Bool
    public var sessionsRunning: Bool

    public init(lastUnlock: Date? = nil, backgroundedAt: Date? = nil, locked: Bool = false, sessionsRunning: Bool = false) {
        self.lastUnlock = lastUnlock
        self.backgroundedAt = backgroundedAt
        self.locked = locked
        self.sessionsRunning = sessionsRunning
    }
}

/// The one place that turns scene phases into Face ID prompts and session starts and stops. Pure.
///
/// - `.background` records the time, stops the sessions and then suspends the transport.
/// - `.inactive` (the Face ID sheet, the notification shade) records nothing and does nothing.
/// - `.active` asks `FaceIDRule`: when Face ID is required the content stays covered and sessions wait for the unlock;
///   otherwise the sessions start.
/// - Background refresh never asks for Face ID.
public enum LifecyclePlanner {
    public struct Plan: Sendable, Equatable {
        public let actions: [LifecycleAction]
        public let state: LifecycleState
    }

    public static func phase(_ phase: PhonePhase, state: LifecycleState, faceIDOn: Bool, now: Date) -> Plan {
        var next = state
        switch phase {
        case .inactive:
            return Plan(actions: [.noop], state: state)
        case .background:
            next.backgroundedAt = now
            next.sessionsRunning = false
            return Plan(actions: state.sessionsRunning ? [.stopSessionsThenSuspend] : [.noop], state: next)
        case .active:
            let required = FaceIDRule.required(
                settingOn: faceIDOn, lastUnlock: state.lastUnlock, backgroundedAt: state.backgroundedAt, now: now)
            if required {
                next.locked = true
                return Plan(actions: [.lockUI, .promptFaceID], state: next)
            }
            next.backgroundedAt = nil
            var actions: [LifecycleAction] = []
            if state.locked {
                next.locked = false
                actions.append(.unlockUI)
            }
            if !state.sessionsRunning {
                next.sessionsRunning = true
                actions.append(.startSessions)
            }
            return Plan(actions: actions.isEmpty ? [.noop] : actions, state: next)
        }
    }

    /// Face ID passed: the content opens and the sessions start.
    public static func unlocked(state: LifecycleState, now: Date) -> Plan {
        var next = state
        next.lastUnlock = now
        next.backgroundedAt = nil
        next.locked = false
        var actions: [LifecycleAction] = [.unlockUI]
        if !state.sessionsRunning {
            next.sessionsRunning = true
            actions.append(.startSessions)
        }
        return Plan(actions: actions, state: next)
    }

    /// Face ID failed or was cancelled: the content stays covered; the person can ask again.
    public static func unlockFailed(state: LifecycleState) -> Plan {
        var next = state
        next.locked = true
        return Plan(actions: [.lockUI], state: next)
    }

    /// Background refresh is read-only and never asks for Face ID, whatever the setting and the time away.
    public static func backgroundRefresh(state: LifecycleState, now: Date) -> Plan {
        Plan(actions: [.noop], state: state)
    }
}
