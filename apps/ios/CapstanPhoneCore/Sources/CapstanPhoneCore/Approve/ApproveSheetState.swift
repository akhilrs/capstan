import CapstanKit
import Foundation

/// The steps of the approve sheet. Once applies from `choose`; similar goes on to `scope`; auto to `window`, then a
/// `confirmAuto` whose default is Cancel.
public enum ApproveStep: Equatable, Sendable {
    case choose, scope, window, confirmAuto
}

/// The state of the sheet: which step, what is picked, and the request it would send. Pure value logic; the views
/// only render it.
public struct ApproveSheetState: Equatable, Sendable {
    public let plan: ApprovePlan
    public private(set) var step: ApproveStep = .choose
    public private(set) var top: ApproveTop?
    public private(set) var scope: ApproveScope?
    public private(set) var window: ApproveWindow?

    public init(plan: ApprovePlan) {
        self.plan = plan
        // A single offer is picked already.
        if plan.tops.count == 1 { top = plan.tops[0] }
    }

    // MARK: Picking

    public mutating func pick(_ top: ApproveTop) {
        guard step == .choose, plan.tops.contains(top) else { return }
        self.top = top
    }

    public mutating func pick(_ scope: ApproveScope) {
        guard step == .scope, plan.scopes.contains(scope) else { return }
        self.scope = scope
    }

    public mutating func pick(_ window: ApproveWindow) {
        guard step == .window, plan.windows.contains(window) else { return }
        self.window = window
    }

    // MARK: Steps

    /// The number of steps of the path chosen so far (1 for once, 2 for similar, 3 for auto).
    public var total: Int {
        switch top {
        case nil, .once?: return 1
        case .similar?: return 2
        case .auto?: return 3
        }
    }

    public var number: Int {
        switch step {
        case .choose: return 1
        case .scope, .window: return 2
        case .confirmAuto: return 3
        }
    }

    /// "1 of 3"; nil while the path is the single step of once, which shows no count.
    public var stepText: String? {
        guard total > 1 else { return nil }
        return ApproveStrings.stepOf.replacingOccurrences(of: "{n}", with: String(number))
            .replacingOccurrences(of: "{total}", with: String(total))
    }

    /// The label of the primary button.
    public var primaryLabel: String {
        switch step {
        case .choose:
            switch top {
            case nil: return ApproveStrings.choicesNone
            case .once?: return ApproveStrings.onceConfirmAction
            case .similar?, .auto?: return ApproveStrings.autoNext
            }
        case .scope: return ApproveStrings.scopeAction
        case .window: return ApproveStrings.autoNext
        case .confirmAuto: return ApproveStrings.autoConfirmAction
        }
    }

    /// The primary can be pressed.
    public var primaryEnabled: Bool {
        switch step {
        case .choose: return top != nil
        case .scope: return scope != nil
        case .window: return window != nil
        case .confirmAuto: return window != nil
        }
    }

    /// Moves on when the primary does not send anything; true when it did.
    public mutating func advance() -> Bool {
        guard primaryEnabled else { return false }
        switch (step, top) {
        case (.choose, .similar?): step = .scope
        case (.choose, .auto?): step = .window
        case (.window, _): step = .confirmAuto
        default: return false
        }
        return true
    }

    public mutating func back() {
        switch step {
        case .choose: break
        case .scope, .window: step = .choose
        case .confirmAuto: step = .window
        }
    }

    /// The verb the primary sends at this step; nil when it only moves on (or is not enabled).
    public var request: DecideVerb? {
        guard primaryEnabled else { return nil }
        switch (step, top) {
        case (.choose, .once?): return .opApproveAs(.once)
        case (.scope, _): return scope.map { .opApproveAs($0.choice) }
        case (.confirmAuto, _): return window.map { .opApproveAs($0.choice) }
        default: return nil
        }
    }
}
