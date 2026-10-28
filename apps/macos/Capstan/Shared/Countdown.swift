import SwiftUI

/// "Retrying in N s", counting down once a second. The text changes; nothing moves.
struct Countdown: View {
    let until: Date

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let seconds = max(0, Int(until.timeIntervalSince(context.date).rounded(.up)))
            let line = "Retrying in " + String(seconds) + " s"
            Text(verbatim: line).monospacedDigit()
        }
    }
}

#Preview("Countdown") {
    Countdown(until: Date().addingTimeInterval(30)).padding()
}
