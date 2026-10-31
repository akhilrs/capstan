import CapstanAppCore
import CapstanKit
import SwiftUI

extension View {
    /// The screens a tab's stack can push. Each screen package fills in its route.
    func routes() -> some View {
        navigationDestination(for: PhoneRoute.self) { route in RouteView(route: route) }
    }
}

/// A pushed screen by route. Until the screen packages for items (`scr-detail`), projects (`scr-overview`), agents,
/// plans, conversations and hosts arrive, an item shows its summary as plain untrusted text and the others say what
/// they are.
struct RouteView: View {
    @Environment(AppModel.self) private var model
    let route: PhoneRoute

    var body: some View {
        switch route {
        case .item(let row):
            if let item = model.item(row) {
                ScrollView {
                    VStack(alignment: .leading, spacing: Tokens.space[3]) {
                        Text(verbatim: RowText.inspectorTitle(kind: item.kind, key: item.key, summary: item.summary.displayPlain))
                            .font(TypeStyle.title2)
                        UntrustedTextBlock(text: item.summary, label: item.kind == .op ? "Command" : "Summary")
                    }
                    .padding(Tokens.margin)
                }
                .background(Tokens.color(.ground))
            } else {
                ContentUnavailableView("This item is no longer waiting", systemImage: "checkmark.circle")
            }
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
