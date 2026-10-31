import CapstanAppCore
import CapstanKit
import SwiftUI

extension View {
    /// The screens a tab's stack can push. Each screen package fills in its route.
    func routes() -> some View {
        navigationDestination(for: PhoneRoute.self) { route in RouteView(route: route) }
    }
}

/// A pushed screen by route. Items show their details (`scr-detail`), hosts their details and a project that is not
/// ready its state and fix (`scr-hosts`); until the packages for a ready project (`scr-overview`), agents, plans and
/// conversations arrive, the others say what they are.
struct RouteView: View {
    @Environment(AppModel.self) private var model
    let route: PhoneRoute

    var body: some View {
        switch route {
        case .item(let row):
            ItemDetailScreen(row: row)
        case .project(let hostId, let projectId)
            where model.project(hostId, projectId).map({ $0.availability != .ready }) ?? true:
            ProjectStateScreen(hostId: hostId, projectId: projectId)
        case .host(let hostId):
            HostDetailScreen(hostId: hostId)
        case .project(let hostId, let projectId):
            ContentUnavailableView {
                Label { Text(verbatim: projectId) } icon: { Image(systemName: "folder") }
            } description: {
                Text(verbatim: model.hosts.first(where: { $0.id == hostId })?.displayName ?? "")
            }
        case .agent, .plan, .conversation:
            ContentUnavailableView("Coming soon", systemImage: "hammer")
        }
    }
}
