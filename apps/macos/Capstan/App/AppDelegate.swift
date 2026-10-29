import AppKit
import UserNotifications

/// Owns the app's one model. It is made in `init`, before launch, so every scene reads the same instance.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model: AppModel

    override init() {
        model = AppModel()
        super.init()
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        UNUserNotificationCenter.current().delegate = model.notifications
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        model.start()
    }

    /// Quitting must wait for the engines to flush, so the reply is deferred until `stopAndWait` returns. A 3 s timer
    /// replies anyway, so a stuck engine cannot keep the app from quitting.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        var replied = false
        let reply = {
            guard !replied else { return }
            replied = true
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        Task { @MainActor in
            await model.stopAndWait()
            reply()
        }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(3))
            reply()
        }
        return .terminateLater
    }
}
