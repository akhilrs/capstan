import SwiftUI

/// "Retrying in N s", counting down once a second. The text changes; nothing moves.
struct Countdown: View {
    let until: Date

    var body: some View {
        TickingText { now in
            "Retrying in " + String(max(0, Int(until.timeIntervalSince(now).rounded(.up)))) + " s"
        }
    }
}

/// Text that changes every second without changing size: only this text is redrawn on each tick, its digits are
/// monospaced, and a hidden copy with every number padded to two digits reserves the width of its longest form.
struct TickingText: View {
    let line: (Date) -> String

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let text = line(context.date)
            ZStack(alignment: .leading) {
                Text(verbatim: Self.reserve(text)).hidden()
                Text(verbatim: text)
            }
            .monospacedDigit()
            .lineLimit(1)
            .fixedSize()
        }
    }

    /// `text` with every run of digits padded with leading zeros to two digits ("3 s ago" becomes "03 s ago").
    static func reserve(_ text: String) -> String {
        var out = ""
        var run = ""
        func flush() {
            out += String(repeating: "0", count: max(0, 2 - run.count)) + run
            run = ""
        }
        for ch in text {
            if ch.isASCII && ch.isNumber {
                run.append(ch)
            } else {
                flush()
                out.append(ch)
            }
        }
        flush()
        return out
    }
}

#Preview("Countdown") {
    Countdown(until: Date().addingTimeInterval(30)).padding()
}
