import CapstanKit
import Foundation

/// The "Remove from list" action of a project: when it is offered and what the small alert says.
public enum ProjectRemovalPrompt {
    public static let menuTitle = "Remove from list"
    public static let confirmTitle = "Remove"

    /// Offered for a project the host cannot reach (gone or silent); never for a live one.
    public static func isOffered(_ availability: ProjectAvailability) -> Bool {
        availability.isRemovable
    }

    public static func title(name: String) -> String {
        "Remove \(name) from the list?"
    }

    public static let message =
        "The app forgets this project and stops following it. It comes back by itself if its controller is found again. Nothing changes on the host."

    /// What the person is told when the engine refuses.
    public static func refusal(_ result: ProjectRemoval) -> String? {
        switch result {
        case .removed: return nil
        case .refusedLive: return "This project is live and cannot be removed."
        case .notFound: return "This project is not in the list any more."
        }
    }
}
