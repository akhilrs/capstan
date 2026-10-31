import SwiftUI
import WidgetKit

/// The widget extension: the "N waiting" Live Activity now, the Home Screen widgets later (package `widgets`).
@main
struct CapstanWidgetsBundle: WidgetBundle {
    var body: some Widget {
        WaitingLiveActivity()
    }
}
