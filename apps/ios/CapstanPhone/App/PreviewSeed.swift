import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation

/// The state a preview hands to `AppModel(preview:)`: no session, no engine and no file is made from it.
struct PreviewSeed {
    var hosts: [HostConfig]
    var phoneHosts: [PhoneHost]
    var hostModels: [UUID: HostModel]
    var connection: [UUID: ConnectionState]
    var capabilities: [UUID: HostCapabilities]
    var settings: PhoneSettings
    var key: PhoneKeyState
    var problems: [UUID: HostProblem]
    var pendingTrust: [UUID: PendingTrust]
    var messages = SentMessageLog()
    var pins: [UUID: HostKeyPin]

    init(
        hosts: [HostConfig] = [], phoneHosts: [PhoneHost] = [], hostModels: [UUID: HostModel] = [:],
        connection: [UUID: ConnectionState] = [:], capabilities: [UUID: HostCapabilities] = [:],
        settings: PhoneSettings = PhoneSettings(), key: PhoneKeyState = .none, problems: [UUID: HostProblem] = [:],
        pendingTrust: [UUID: PendingTrust] = [:], pins: [UUID: HostKeyPin] = [:]
    ) {
        self.hosts = hosts
        self.phoneHosts = phoneHosts
        self.hostModels = hostModels
        self.connection = connection
        self.capabilities = capabilities
        self.settings = settings
        self.key = key
        self.problems = problems
        self.pendingTrust = pendingTrust
        self.pins = pins
    }
}
