import CapstanAppCore
import CapstanKit
import SwiftUI

/// The choices of a question an agent sent, one full-width button each. A click only fills the answer: the person still
/// sends it, and may edit it or write something else. The choices are host text, shown plain.
struct ChoiceButtons: View {
    let options: [String]
    /// The answer as it stands; the choice it equals is shown picked.
    let current: String
    let enabled: Bool
    let pick: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                let shown = TextBudget.displayPrefix(option, maxBytes: ProtocolLimits.optionTextCap).text
                let picked = current.trimmingCharacters(in: .whitespacesAndNewlines) == option
                Button {
                    pick(option)
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                        Text(verbatim: String(index + 1))
                            .font(.system(size: 12, weight: .semibold).monospacedDigit())
                            .foregroundStyle(picked ? Tokens.color(.onAccent) : Tokens.color(.label2))
                        Text(verbatim: shown)
                            .font(.system(size: 13))
                            .foregroundStyle(picked ? Tokens.color(.onAccent) : Tokens.color(.label))
                            .multilineTextAlignment(.leading)
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 0)
                        if picked {
                            Image(systemName: "checkmark")
                                .foregroundStyle(Tokens.color(.onAccent))
                                .accessibilityHidden(true)
                        }
                    }
                    .padding(.horizontal, Tokens.space[2])
                    .padding(.vertical, Tokens.space[0] + 2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        picked ? Tokens.color(.accent) : Tokens.color(.fill),
                        in: RoundedRectangle(cornerRadius: Tokens.radius.control))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!enabled)
                .accessibilityAddTraits(picked ? .isSelected : [])
            }
        }
    }
}

#Preview("ChoiceButtons") {
    ChoiceButtons(options: ["Ship it", "Wait for review", "Cancel the plan"], current: "Ship it", enabled: true) { _ in }
        .padding()
        .frame(width: 360)
}
