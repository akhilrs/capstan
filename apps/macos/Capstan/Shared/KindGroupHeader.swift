import CapstanKit
import SwiftUI

/// The title of one group of the inbox, with its count.
struct KindGroupHeader: View {
    let kind: WaitingKind
    let count: Int

    var body: some View {
        HStack {
            Text(verbatim: title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Tokens.color(.label2))
            Spacer()
            Text(verbatim: String(count)).font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
        }
        .accessibilityElement(children: .combine)
    }

    private var title: String {
        switch kind {
        case .op: return "Command proposals"
        case .prompt: return "Permission prompts"
        case .design: return "Design approvals"
        case .message: return "Messages"
        case .integration: return "Ready to merge"
        case .other: return "Other"
        }
    }
}

#Preview("KindGroupHeader") {
    VStack {
        KindGroupHeader(kind: .op, count: 3)
        KindGroupHeader(kind: .prompt, count: 1)
        KindGroupHeader(kind: .integration, count: 12)
    }
    .padding()
    .frame(width: 320)
}
