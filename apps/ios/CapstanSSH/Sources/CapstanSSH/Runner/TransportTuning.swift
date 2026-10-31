import CapstanKit
import Foundation
import NIOCore

/// The transport's timings. Keepalive is applied as socket options by the hook; it is never timed here.
public struct TransportTuning: Sendable, Equatable {
    public var connectTimeout: TimeInterval
    /// How long a connection stays open after its last channel ended.
    public var idleClose: TimeInterval
    /// TCP keepalive: seconds idle before the first probe, seconds between probes, probes before the link is dead.
    public var keepaliveIdle: Int32
    public var keepaliveInterval: Int32
    public var keepaliveCount: Int32

    public init(
        connectTimeout: TimeInterval = 10, idleClose: TimeInterval = 60, keepaliveIdle: Int32 = 15,
        keepaliveInterval: Int32 = 15, keepaliveCount: Int32 = 3
    ) {
        self.connectTimeout = connectTimeout
        self.idleClose = idleClose
        self.keepaliveIdle = keepaliveIdle
        self.keepaliveInterval = keepaliveInterval
        self.keepaliveCount = keepaliveCount
    }
}

/// One socket option set at bootstrap.
public struct SocketOption: Sendable, Equatable {
    public var level: Int32
    public var name: Int32
    public var value: Int32

    public init(level: Int32, name: Int32, value: Int32) {
        self.level = level
        self.name = name
        self.value = value
    }
}

/// Algorithm names in order of preference. The runner uses what the library lets it set and checks the rest against
/// the server's offer.
public struct AlgorithmPreferences: Sendable, Equatable {
    public var hostKeys: [String]
    public var kex: [String]
    public var ciphers: [String]

    public init(hostKeys: [String], kex: [String], ciphers: [String]) {
        self.hostKeys = hostKeys
        self.kex = kex
        self.ciphers = ciphers
    }
}

/// What swift-nio-ssh 0.15.1 does, in its own order (see apps/ios/Spikes/NIOSSHSpike). Neither the key exchange nor
/// the host-key order can be changed; ciphers can be narrowed.
public enum SupportedAlgorithms {
    public static let kex = [
        "ecdh-sha2-nistp384", "ecdh-sha2-nistp256", "ecdh-sha2-nistp521", "curve25519-sha256",
        "curve25519-sha256@libssh.org",
    ]
    public static let hostKeys = ["ssh-ed25519", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp521"]
    public static let ciphers = ["aes256-gcm@openssh.com", "aes128-gcm@openssh.com"]

    public static var library: AlgorithmPreferences {
        AlgorithmPreferences(hostKeys: hostKeys, kex: kex, ciphers: ciphers)
    }
}

/// The transport's tuning points. `ssh-tuning` implements it; `DefaultHook` is the library as it is.
public protocol TransportHook: Sendable {
    /// Socket options set on every connection.
    func channelOptions() -> [SocketOption]
    /// The algorithms to use with a host; `pinnedHostKey` is the algorithm of its pin, if any.
    func algorithms(pinnedHostKey: String?) -> AlgorithmPreferences
    /// Ends one exec channel; the future succeeds once the close is under way.
    func terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void>
}

/// No socket options, the library's algorithms, and terminate by closing the channel.
public struct DefaultHook: TransportHook {
    public init() {}

    public func channelOptions() -> [SocketOption] {
        []
    }

    public func algorithms(pinnedHostKey: String?) -> AlgorithmPreferences {
        SupportedAlgorithms.library
    }

    public func terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void> {
        channel.close()
    }
}
