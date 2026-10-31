import SwiftUI

/// The counts on the tab badges. Zero shows no badge.
struct TabCounts: Equatable {
    var waiting = 0
    var messages = 0
}

/// Five tabs, each with its own navigation stack bound to the model's path for that tab, so deep links and menus can
/// move between them. Screen packages replace the placeholder roots.
struct RootTabView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        let counts = model.tabCounts
        TabView(selection: $model.navigation.tab) {
            Tab("Waiting", systemImage: "tray", value: PhoneTab.waiting) {
                NavigationStack(path: path(.waiting)) {
                    WaitingScreen().routes()
                }
            }
            .badge(counts.waiting)

            Tab("Agents", systemImage: "person.2", value: PhoneTab.agents) {
                NavigationStack(path: path(.agents)) {
                    AgentsScreen().routes()
                }
            }

            Tab("Plans", systemImage: "list.bullet.rectangle", value: PhoneTab.plans) {
                NavigationStack(path: path(.plans)) {
                    PlansScreen().routes()
                }
            }

            Tab("Messages", systemImage: "bubble.left", value: PhoneTab.messages) {
                NavigationStack(path: path(.messages)) {
                    ContentUnavailableView(
                        "No messages", systemImage: "bubble.left",
                        description: Text("Your conversations with the PM appear here."))
                        .navigationTitle("Messages")
                        .routes()
                }
            }
            .badge(counts.messages)

            Tab("Hosts", systemImage: "server.rack", value: PhoneTab.hosts) {
                NavigationStack(path: path(.hosts)) {
                    HostsScreen().routes()
                }
            }
        }
        .hostSheets(model)
        .tabBarMinimizeBehavior(.onScrollDown)
        .tint(Tokens.color(.accent))
    }

    private func path(_ tab: PhoneTab) -> Binding<[PhoneRoute]> {
        Binding(get: { model.navigation.path(tab) }, set: { model.navigation.paths[tab] = $0 })
    }
}

#Preview("RootTabView") {
    RootTabView().environment(AppModel(preview: PreviewData.standard))
}
