import CapstanKit
import Foundation

/// The words of a car announcement (plan-156 design-voice, `strings.json`, the `notify.*` keys): the kind and the
/// project only. Nothing here carries host or agent text; the project name is added by the plan, plain and capped.
public enum CarTexts {
    /// The kind keys of `strings.json`, in design order.
    public static let kindKeys = ["op", "prompt", "design", "message", "integration", "other"]

    /// The placeholder the project name replaces in a body.
    public static let projectPlaceholder = "{project}"

    /// The key of a kind in `strings.json` (`notify.<key>.title`).
    public static func key(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "op"
        case .prompt: return "prompt"
        case .design: return "design"
        case .message: return "message"
        case .integration: return "integration"
        case .other: return "other"
        }
    }

    public static func title(_ kind: WaitingKind) -> String {
        switch kind {
        case .op: return "Command to approve"
        case .prompt: return "Permission prompt to answer"
        case .design: return "Design to review"
        case .message: return "Message to answer"
        case .integration: return "Ready to merge"
        case .other: return "Item waiting"
        }
    }

    /// The body with the placeholder, the same for every kind.
    public static let bodyTemplate = "In {project}."

    public static func body(project: String) -> String {
        bodyTemplate.replacingOccurrences(of: projectPlaceholder, with: project)
    }

    /// Every announcement text by its `strings.json` key, for the drift test.
    public static var announcementKeys: [String: String] {
        var all: [String: String] = [:]
        for key in kindKeys {
            let kind = WaitingKind(wire: key)
            all["notify.\(key).title"] = title(kind)
            all["notify.\(key).body"] = bodyTemplate
        }
        return all
    }
}
