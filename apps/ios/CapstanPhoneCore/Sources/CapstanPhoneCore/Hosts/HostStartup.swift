import CapstanKit
import Foundation

/// The host part of the launch sequence: load both host lists, drop what a kill between two writes left
/// half-saved, register the saved hosts and sweep the pins of hosts that are gone.
///
/// `addHost` saves `PhoneHostStore` first and `HostsFile` second; a kill between the two leaves a `PhoneHost` without a
/// `HostConfig`, or the reverse after a failed remove. Either is dropped here and the cleaned lists are saved.
public enum HostStartup {
    /// The two lists with every host that has no partner (or does not register) removed.
    public static func reconcile(
        configs: [HostConfig], phoneHosts: [PhoneHost]
    ) -> (configs: [HostConfig], phoneHosts: [PhoneHost]) {
        let configIds = Set(configs.map(\.id))
        let phoneIds = Set(phoneHosts.map(\.id))
        return (configs.filter { phoneIds.contains($0.id) }, phoneHosts.filter { configIds.contains($0.id) })
    }

    /// Runs the host part of the launch and returns the hosts to start sessions for.
    public static func run(
        hostsFile: HostsFile, phoneStore: PhoneHostStore, registry: PhoneHostRegistry, pins: HostKeyStore
    ) -> [HostConfig] {
        let loadedConfigs = hostsFile.load()
        let loadedPhoneHosts = phoneStore.load()
        var (configs, phoneHosts) = reconcile(configs: loadedConfigs, phoneHosts: loadedPhoneHosts)

        // A saved host that does not register (invalid, or a duplicate of an earlier one) is dropped with its config.
        var registered: Set<UUID> = []
        for host in phoneHosts {
            do {
                try registry.register(host)
                registered.insert(host.id)
            } catch {
                continue
            }
        }
        phoneHosts = phoneHosts.filter { registered.contains($0.id) }
        configs = configs.filter { registered.contains($0.id) }

        if phoneHosts.count != loadedPhoneHosts.count { try? phoneStore.save(phoneHosts) }
        if configs.count != loadedConfigs.count { try? hostsFile.save(configs) }
        pins.sweep(keeping: registered)
        return configs
    }
}
