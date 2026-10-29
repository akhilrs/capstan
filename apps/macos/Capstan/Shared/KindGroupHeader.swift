import SwiftUI

/// The title of one group of the inbox with its count, such as `Command proposals · 2`.
struct KindGroupHeader: View {
    let title: String
    var quiet = false

    var body: some View {
        Text(verbatim: title)
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Tokens.color(.label2))
            .opacity(quiet ? 0.8 : 1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityAddTraits(.isHeader)
    }
}

#Preview("KindGroupHeader") {
    VStack {
        KindGroupHeader(title: "Command proposals · 3")
        KindGroupHeader(title: "Ready to merge · 1")
        KindGroupHeader(title: "Merge conflicts · 2", quiet: true)
    }
    .padding()
    .frame(width: 320)
}
