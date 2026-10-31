import CapstanAppCore
import CapstanKit
import SwiftUI

extension View {
    /// The screens a tab's stack can push. Each screen package fills in its route.
    func routes() -> some View {
        navigationDestination(for: PhoneRoute.self) { route in RouteView(route: route) }
    }
}

/// A pushed screen by route: item details, host details, a project (its overview when ready, else its state and
/// fix), agents, plans and Settings. Conversations arrive with `scr-messages` (R3).
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
        case .settings:
            SettingsScreen()
        case .project(let hostId, let projectId):
            ProjectOverviewScreen(hostId: hostId, projectId: projectId)
        case .agent(let hostId, let projectId, let agentId):
            AgentDetailScreen(hostId: hostId, projectId: projectId, agentId: agentId)
        case .plan(let ref):
            PlanDetailScreen(ref: ref)
        case .conversation(let hostId, let projectId):
            ConversationScreen(hostId: hostId, projectId: projectId)
        }
    }
}
