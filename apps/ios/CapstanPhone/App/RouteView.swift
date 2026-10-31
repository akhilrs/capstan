import CapstanAppCore
import CapstanKit
import SwiftUI

extension View {
    /// The screens a tab's stack can push. Each screen package fills in its route.
    func routes() -> some View {
        navigationDestination(for: PhoneRoute.self) { route in RouteView(route: route) }
    }
}

/// A pushed screen by route. Items show their details (`scr-detail`); until the screen packages for projects
/// (`scr-overview`), agents, plans, conversations and hosts arrive, the others say what they are.
struct RouteView: View {
    @Environment(AppModel.self) private var model
    let route: PhoneRoute

    var body: some View {
        switch route {
        case .item(let row):
            ItemDetailScreen(row: row)
        case .project(let hostId, let projectId):
            ContentUnavailableView {
                Label { Text(verbatim: projectId) } icon: { Image(systemName: "folder") }
            } description: {
                Text(verbatim: model.hosts.first(where: { $0.id == hostId })?.displayName ?? "")
            }
        case .agent, .plan, .conversation, .host:
            ContentUnavailableView("Coming soon", systemImage: "hammer")
        }
    }
}
