import SwiftUI

@main
struct CapstanApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup(id: "main") {
            MainWindow(model: delegate.model)
                .tint(Tokens.color(.accent))
                .accentColor(Tokens.color(.accent))
        }
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Refresh") {
                    delegate.model.refreshSelection()
                }
                .keyboardShortcut("r", modifiers: .command)
            }
        }

        Settings {
            SettingsScreen(model: delegate.model)
                .tint(Tokens.color(.accent))
                .accentColor(Tokens.color(.accent))
        }

        MenuBarExtra {
            MenuBarContent(model: delegate.model)
                .tint(Tokens.color(.accent))
                .accentColor(Tokens.color(.accent))
        } label: {
            MenuBarLabel(model: delegate.model, count: delegate.model.badgeCount)
        }
        .menuBarExtraStyle(.window)
    }
}
