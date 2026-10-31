import ActivityKit
import CapstanPhoneCore

/// The Live Activity's attributes. ActivityKit matches the app's and the extension's copies by name, so this type is
/// written twice, identically: `CapstanPhone/System/WaitingActivityAttributes.swift` and
/// `CapstanWidgets/LiveActivity/WaitingActivityAttributes.swift` (CapstanPhoneCore may not import ActivityKit).
/// No static attributes: everything shown is in `WaitingActivityState`, counts and times only.
struct WaitingActivityAttributes: ActivityAttributes {
    typealias ContentState = WaitingActivityState
}
