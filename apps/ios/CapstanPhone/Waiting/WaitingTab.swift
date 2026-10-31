import SwiftUI

/// The Waiting tab at both widths (board 16). Compact (portrait, every iPhone): one stack. Regular (Pro Max and Plus
/// in landscape): a split view with the list beside the item, so the same path shows in the detail column.
struct WaitingTab: View {
    @Environment(\.horizontalSizeClass) private var sizeClass
    let path: Binding<[PhoneRoute]>

    var body: some View {
        if sizeClass == .regular {
            NavigationSplitView {
                WaitingScreen()
                    .navigationSplitViewColumnWidth(min: 320, ideal: 380, max: 440)
            } detail: {
                NavigationStack(path: path) {
                    ContentUnavailableView(
                        "Choose an item", systemImage: "tray",
                        description: Text("What waits for you shows here, beside the list."))
                        .background(Tokens.color(.ground))
                        .routes()
                }
            }
            .navigationSplitViewStyle(.balanced)
        } else {
            NavigationStack(path: path) {
                WaitingScreen().routes()
            }
        }
    }
}
