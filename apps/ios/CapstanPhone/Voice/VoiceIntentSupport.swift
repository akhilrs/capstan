import AppIntents
import CapstanPhoneCore
import Foundation

/// What the four intents share: the app's environment for a run, and the spoken answer. An intent only calls
/// `VoiceFlow.run`; it never reads a host or sends anything itself.
enum VoiceRun {
    /// The lines as one dialog: each line is one sentence.
    static func dialog(_ lines: [String]) -> IntentDialog {
        IntentDialog(LocalizedStringResource(stringLiteral: lines.joined(separator: " ")))
    }
}

/// A value the intent hands to a closure that runs inside the same intent call.
struct IntentBox<Value>: @unchecked Sendable {
    let value: Value
}
