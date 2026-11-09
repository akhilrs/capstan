import Foundation

/// The fixed app text for the source of an agent's model (`modelSource`). The host's string is never shown: any
/// value outside the six layers of the chain, and an absent one, reads "other".
public struct ModelLabel: Sendable, Equatable {
    /// The word in a list row.
    public let short: String
    /// The label in an inspector or a detail.
    public let long: String
    /// What VoiceOver says.
    public let spoken: String

    public static func label(source: String?) -> ModelLabel {
        switch source {
        case "spawn": return ModelLabel(short: "spawn", long: "Chosen at spawn", spoken: "chosen at spawn")
        case "task": return ModelLabel(short: "task", long: "Task override", spoken: "task override")
        case "plan": return ModelLabel(short: "plan", long: "Plan override", spoken: "plan override")
        case "role": return ModelLabel(short: "role", long: "Role setting", spoken: "role setting")
        case "host": return ModelLabel(short: "host", long: "Host setting", spoken: "host setting")
        case "defaults": return ModelLabel(short: "defaults", long: "Config defaults", spoken: "config defaults")
        default: return ModelLabel(short: "other", long: "Other source", spoken: "other source")
        }
    }
}
