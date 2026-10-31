import Foundation

/// Why the transport could not use what a host offered.
public enum UnsupportedReason: Sendable, Equatable {
    /// The host's key is of a type the client cannot verify.
    case hostKeyType(String)
    /// No key exchange, cipher or host-key algorithm in common.
    case noCommonAlgorithm
}

/// Something the transport needs the person to see. Host text is never carried here.
public enum TransportEvent: Sendable, Equatable {
    case hostKeyUnknown(hostId: UUID, pin: HostKeyPin)
    case hostKeyChanged(hostId: UUID)
    case authRefused(hostId: UUID)
    case unsupported(hostId: UUID, UnsupportedReason)
    case pinsUnreadable
}

/// A host key waiting for the person's Trust or Cancel.
public struct PendingTrust: Sendable, Equatable {
    public let hostId: UUID
    public let pin: HostKeyPin

    public init(hostId: UUID, pin: HostKeyPin) {
        self.hostId = hostId
        self.pin = pin
    }
}

/// Restarts a host's sessions, after the person trusted its key. The app's coordinator implements it.
@MainActor
public protocol SessionControl: AnyObject {
    func restart(hostId: UUID)
}

/// The transport's side: its events, the latest one per host, and stopping before the app is suspended.
/// A host's latest event clears on its next successful start, `trust()` or `forget`. `SSHRunner` implements it.
public protocol TransportControl: Sendable {
    var events: AsyncStream<TransportEvent> { get }
    func latestEvent(hostId: UUID) -> TransportEvent?
    func suspend()
}
