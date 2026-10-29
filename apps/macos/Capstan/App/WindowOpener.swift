import SwiftUI

private struct WindowOpener: ViewModifier {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        content.onAppear {
            model.openMainWindow = { openWindow(id: "main") }
        }
    }
}

extension View {
    /// Gives the model a way to open the main window from the `openWindow` action of this view's scene.
    func registersWindowOpener(_ model: AppModel) -> some View {
        modifier(WindowOpener(model: model))
    }
}
