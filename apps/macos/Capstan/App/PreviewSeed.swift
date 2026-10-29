import CapstanAppCore
import CapstanKit
import Foundation

/// The state a preview or a test hands to `AppModel(preview:)`: no session, no engine and no file is made from it.
struct PreviewSeed {
    var hosts: [HostConfig]
    var hostModels: [UUID: HostModel]
    var connection: [UUID: ConnectionState]
    var capabilities: [UUID: HostCapabilities]
    var settings: AppSettings

    init(
        hosts: [HostConfig] = [], hostModels: [UUID: HostModel] = [:], connection: [UUID: ConnectionState] = [:],
        capabilities: [UUID: HostCapabilities] = [:], settings: AppSettings = AppSettings()
    ) {
        self.hosts = hosts
        self.hostModels = hostModels
        self.connection = connection
        self.capabilities = capabilities
        self.settings = settings
    }
}
