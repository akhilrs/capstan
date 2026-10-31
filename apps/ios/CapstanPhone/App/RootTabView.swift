import SwiftUI

/// The counts on the tab badges. Zero shows no badge. Package `app-model` binds them to the model.
struct TabCounts: Equatable {
    var waiting = 0
    var messages = 0
}

/// Five tabs, each with its own navigation stack. The screen packages replace the placeholder roots.
struct RootTabView: View {
    var counts = TabCounts()

    var body: some View {
        TabView {
            Tab("Waiting", systemImage: "tray") {
                NavigationStack {
                    ContentUnavailableView(
                        "Nothing waits", systemImage: "tray",
                        description: Text("What your agents wait for appears here."))
                        .navigationTitle("Waiting")
                }
            }
            .badge(counts.waiting)

            Tab("Agents", systemImage: "person.2") {
                NavigationStack {
                    ContentUnavailableView(
                        "No agents", systemImage: "person.2",
                        description: Text("The agents of your projects appear here."))
                        .navigationTitle("Agents")
                }
            }

            Tab("Plans", systemImage: "list.bullet.rectangle") {
                NavigationStack {
                    ContentUnavailableView(
                        "No plans", systemImage: "list.bullet.rectangle",
                        description: Text("The plans of your projects appear here."))
                        .navigationTitle("Plans")
                }
            }

            Tab("Messages", systemImage: "bubble.left") {
                NavigationStack {
                    ContentUnavailableView(
                        "No messages", systemImage: "bubble.left",
                        description: Text("Your conversations with the PM appear here."))
                        .navigationTitle("Messages")
                }
            }
            .badge(counts.messages)

            Tab("Hosts", systemImage: "server.rack") {
                NavigationStack {
                    ContentUnavailableView(
                        "No hosts", systemImage: "server.rack",
                        description: Text("Add a host to see its projects."))
                        .navigationTitle("Hosts")
                }
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .tint(Tokens.color(.accent))
    }
}

#Preview("RootTabView") {
    RootTabView(counts: TabCounts(waiting: 7, messages: 1))
}
