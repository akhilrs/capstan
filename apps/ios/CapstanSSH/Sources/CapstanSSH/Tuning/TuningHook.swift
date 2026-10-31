import CapstanKit
import Foundation
import NIOCore
import NIOSSH

/// The transport's tuning (plan package `ssh-tuning`), from the `ssh-spike` results:
/// - TCP keepalive closes a dead link in about `idle + interval × count` seconds (60 with the defaults); channels then
///   end with 255. `HostSession`'s heartbeat check stays the main detector.
/// - The algorithm list is what the library does, with the pinned host-key type first. swift-nio-ssh 0.15.1 does not
///   let a client order its host-key algorithms, so a host whose pin is of another type than the client's first match
///   still reads as changed: a possible false alarm, on the safe side.
/// - `terminate` sends signal TERM and closes the channel after the grace if the command has not ended by then.
public struct TuningHook: TransportHook {
    private let tuning: TransportTuning

    public init(tuning: TransportTuning = TransportTuning()) {
        self.tuning = tuning
    }

    public static let kex = ["curve25519-sha256", "ecdh-sha2-nistp256", "ecdh-sha2-nistp384", "ecdh-sha2-nistp521"]
    public static let hostKeys = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521"]
    public static let ciphers = ["aes128-gcm@openssh.com", "aes256-gcm@openssh.com"]

    public func channelOptions() -> [SocketOption] {
        [
            SocketOption(level: SOL_SOCKET, name: SO_KEEPALIVE, value: 1),
            SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPALIVE, value: tuning.keepaliveIdle),
            SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPINTVL, value: tuning.keepaliveInterval),
            SocketOption(level: Int32(IPPROTO_TCP), name: TCP_KEEPCNT, value: tuning.keepaliveCount),
        ]
    }

    public func algorithms(pinnedHostKey: String?) -> AlgorithmPreferences {
        var hostKeys = TuningHook.hostKeys
        if let pinnedHostKey, let index = hostKeys.firstIndex(of: pinnedHostKey) {
            hostKeys.remove(at: index)
            hostKeys.insert(pinnedHostKey, at: 0)
        }
        return AlgorithmPreferences(hostKeys: hostKeys, kex: TuningHook.kex, ciphers: TuningHook.ciphers)
    }

    public func terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void> {
        let sent = channel.triggerUserOutboundEvent(SSHChannelRequestEvent.SignalRequest(signal: "TERM"))
        // A channel that cannot take the signal is closed at once; otherwise after the grace, if it is still open.
        sent.whenFailure { _ in channel.close(promise: nil) }
        Task {
            try? await clock.sleep(seconds: max(grace, 0))
            channel.close(promise: nil)
        }
        return sent.recover { _ in }
    }
}
