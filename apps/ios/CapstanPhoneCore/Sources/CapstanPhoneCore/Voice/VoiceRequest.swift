import CapstanKit
import Foundation

public enum VoiceRequestError: Error, Sendable, Equatable {
    /// The choice is not one that `VoiceDesk.choices` lists for this item under these settings.
    case notOffered
}

/// Builds the decision a spoken answer sends. Nothing is built for an answer that was not listed.
public enum VoiceRequest {
    /// The suffix of the client label that marks a decision made by voice.
    public static let clientSuffix = " voice"
    /// The longest client label the host accepts, in UTF-8 bytes.
    public static let clientBytes = 64

    public static func make(
        choice: VoiceChoice, target: VoiceTarget, settings: PhoneSettings, nonce: String
    ) throws -> DecideRequest {
        let listed = VoiceDesk.choices(target.item, settings)
        let verb: DecideVerb
        switch choice {
        case .deny:
            guard listed.canDeny else { throw VoiceRequestError.notOffered }
            verb = .opDeny(note: nil)
        case .forwardedApprove:
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApprove
        case .once:
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApproveAs(.once)
        case .exact:
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApproveAs(.exact)
        case .prefix(let raw, _):
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApproveAs(.prefix(raw: raw))
        case .autoHour:
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApproveAs(.autoHour)
        case .autoSession:
            guard listed.approve.contains(choice) else { throw VoiceRequestError.notOffered }
            verb = .opApproveAs(.autoSession)
        }
        return DecideRequest(
            hostId: target.hostId, projectId: target.projectId, requestId: target.requestId,
            expect: target.contentHash, nonce: nonce, client: client(settings.clientLabel), verb: verb)
    }

    /// The label and " voice", within 64 bytes: the label is cut between characters, never inside one.
    public static func client(_ label: String) -> String {
        let room = clientBytes - clientSuffix.utf8.count
        var kept = ""
        var size = 0
        for character in label {
            let width = String(character).utf8.count
            if size + width > room { break }
            kept.append(character)
            size += width
        }
        return kept + clientSuffix
    }
}
