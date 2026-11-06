import AppIntents

/// The phrases Siri knows. The words are the design's `phrase.*` strings (checked by the design's check.py).
struct CapstanShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: WhatsWaitingIntent(),
            phrases: ["What's waiting in \(.applicationName)"],
            shortTitle: "What's waiting",
            systemImageName: "tray")
        AppShortcut(
            intent: ApproveOnceIntent(),
            phrases: ["Approve once in \(.applicationName)"],
            shortTitle: "Approve once",
            systemImageName: "checkmark.circle")
        AppShortcut(
            intent: ApproveWithGrantIntent(),
            phrases: ["Approve in \(.applicationName)"],
            shortTitle: "Approve",
            systemImageName: "checkmark.seal")
        AppShortcut(
            intent: DenyIntent(),
            phrases: ["Deny in \(.applicationName)"],
            shortTitle: "Deny",
            systemImageName: "xmark.circle")
    }
}
