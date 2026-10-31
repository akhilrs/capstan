import SwiftUI

/// Board 01, Welcome: what the app does and how it connects. Left-aligned, one primary action.
struct WelcomeScreen: View {
    let next: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[4]) {
                Image(systemName: "helm")
                    .font(.system(.largeTitle))
                    .foregroundStyle(Tokens.color(.accent))
                    .accessibilityHidden(true)
                    .padding(.top, Tokens.space[6])
                Text("Answer your agents from your iPhone")
                    .font(TypeStyle.largeTitle)
                    .fixedSize(horizontal: false, vertical: true)
                Text("Capstan connects to your own machines over SSH and follows cstan events. There is no account and no relay: this iPhone talks to each host directly with its own key.")
                    .font(TypeStyle.body)
                    .foregroundStyle(Tokens.color(.label2))
                VStack(spacing: 0) {
                    FeatureRow(title: "Command proposals and prompts", symbol: "terminal")
                    Divider()
                    FeatureRow(title: "Design approvals and merges", symbol: "photo")
                    Divider()
                    FeatureRow(title: "Messages to each project's PM", symbol: "bubble.left")
                }
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            }
            .padding(.horizontal, Tokens.margin)
        }
        .background(Tokens.color(.ground))
        .safeAreaInset(edge: .bottom) {
            ActionBar {
                Button("Set up this iPhone", action: next).buttonStyle(.capstanPrimary)
            }
        }
        .toolbar(.hidden, for: .navigationBar)
    }
}

private struct FeatureRow: View {
    let title: String
    let symbol: String

    var body: some View {
        HStack {
            Text(verbatim: title).font(TypeStyle.body)
            Spacer()
            Image(systemName: symbol).foregroundStyle(Tokens.color(.label2)).accessibilityHidden(true)
        }
        .padding(Tokens.space[3])
        .frame(minHeight: 44)
    }
}

#Preview("Welcome") {
    NavigationStack { WelcomeScreen(next: {}) }
}

#Preview("Welcome, dark") {
    NavigationStack { WelcomeScreen(next: {}) }.preferredColorScheme(.dark)
}
