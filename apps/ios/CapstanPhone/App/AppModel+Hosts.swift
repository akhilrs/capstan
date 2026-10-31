import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import Foundation

/// Hosts, trust, the key and the settings. The coordinator does the lifecycle; this keeps the models in step.
extension AppModel {
    // MARK: Adding and changing hosts

    /// Registers a new host as a candidate for the add-host form; the poll interval comes from Settings.
    func prepareCandidate(
        displayName: String, hostname: String, port: Int, user: String, cstanPath: String = "cstan",
        extraRoots: [String] = []
    ) throws -> HostCandidate? {
        guard let coordinator else { return nil }
        return try coordinator.prepareCandidate(
            displayName: displayName, hostname: hostname, port: port, user: user, cstanPath: cstanPath,
            extraRoots: extraRoots, pollMs: settings.pollMs)
    }

    /// The connection check of a host or a candidate, for the sheet; nothing of it is kept.
    func check(_ config: HostConfig) -> AsyncStream<HostCheck> {
        guard let coordinator else { return AsyncStream { $0.finish() } }
        return coordinator.check(config)
    }

    func addHost(_ candidate: HostCandidate) {
        coordinator?.commit(candidate)
    }

    func discardCandidate(_ candidate: HostCandidate) {
        coordinator?.discard(candidate)
    }

    func updateHost(_ config: HostConfig, host: PhoneHost) throws {
        try coordinator?.updateHost(config, host: host)
    }

    /// Removes a host: its session stops, its pins are forgotten, and its models, saved file and pending decisions go.
    func removeHost(_ id: UUID) {
        coordinator?.removeHost(id)
        generations[id] = (generations[id] ?? 0) + 1
        probes[id]?.cancel()
        probes[id] = nil
        sessions[id] = nil
        engines[id] = nil
        hostModels[id] = nil
        reportedModels[id] = nil
        connection[id] = nil
        capabilities[id] = nil
        forwards = forwards.filter { $0.key.hostId != id }
        navigation.paths = navigation.paths.mapValues { path in path.filter { !AppModel.route($0, names: id) } }
        if let directory { try? FilePersistence(directory: directory).delete(hostId: id) }
        let pendingStore = self.pendingStore
        Task { await pendingStore?.remove(hostId: id) }
    }

    private static func route(_ route: PhoneRoute, names hostId: UUID) -> Bool {
        switch route {
        case .item(let row): return row.hostId == hostId
        case .project(let id, _), .agent(let id, _, _), .conversation(let id, _), .host(let id): return id == hostId
        case .plan(let ref): return ref.hostId == hostId
        case .settings: return false
        }
    }

    // MARK: Sessions

    /// Try again: a refused key or an unsupported host is asked again; a changed key stays blocked.
    func restart(hostId: UUID) {
        guard hosts.contains(where: { $0.id == hostId }) else { return }
        if sessions[hostId] != nil { connection[hostId] = .connecting }
        coordinator?.restart(hostId: hostId)
    }

    /// Reads the ready projects of a host again (pull to refresh).
    func refresh(hostId: UUID, projectIds: Set<String>) {
        guard let session = sessions[hostId], !projectIds.isEmpty else { return }
        Task { await session.refresh(projectIds: projectIds) }
    }

    /// Drops a project the host cannot reach from the list and the cursor; a live project is refused.
    @discardableResult
    func removeProject(hostId: UUID, projectId: String) async -> ProjectRemoval {
        guard let engine = engines[hostId] else { return .notFound }
        let result = await engine.removeProject(projectId)
        guard result == .removed else { return result }
        await sessions[hostId]?.forget(projectIds: [projectId])
        forwards = forwards.filter { !($0.key.hostId == hostId && $0.key.projectId == projectId) }
        return .removed
    }

    // MARK: Trust

    func trust(hostId: UUID) {
        coordinator?.trust(hostId: hostId)
    }

    func cancelTrust(hostId: UUID) {
        coordinator?.cancelTrust(hostId: hostId)
    }

    func resetAllPins() {
        coordinator?.resetAllPins()
    }

    // MARK: Key

    /// Makes the phone's key, or a new one in place of the old (which stays when making the new one fails).
    func makeKey(_ kind: PhoneKeyKind) {
        guard let vault else { return }
        do {
            if case .present = key {
                _ = try vault.replace(kind)
            } else {
                _ = try vault.create(kind)
            }
            key = AppModel.readKey(vault)
        } catch KeyVaultError.enclaveUnavailable {
            key = .unavailable("This device has no Secure Enclave. Use an ed25519 key instead.")
        } catch {
            key = AppModel.readKey(vault)
        }
    }

    // MARK: Settings

    /// Keeps new settings; a changed poll interval applies when a host's session starts again.
    func updateSettings(_ new: PhoneSettings) {
        settings = new
        if let directory { try? PhoneSettingsStore(directory: directory).save(new) }
        publishGlance()
    }
}
