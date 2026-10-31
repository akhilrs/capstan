import CapstanAppCore
import SwiftUI

/// The choices of a question an agent sent (`cstan send --action @user … --option …`), one full-width button each. A
/// tap only fills the answer: the person still sends it, and may edit it or write something else. The choices are
/// host text, shown plain.
struct ChoiceButtons: View {
    let options: [String]
    /// The answer as it stands; the choice it equals is shown picked.
    let current: String
    let enabled: Bool
    let pick: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                let picked = current.trimmingCharacters(in: .whitespacesAndNewlines) == option
                Button {
                    Haptics.play(.selection)
                    pick(option)
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: Tokens.space[2]) {
                        Text(verbatim: String(index + 1))
                            .font(TypeStyle.footnote.monospacedDigit().weight(.semibold))
                            .foregroundStyle(picked ? Tokens.color(.onAccent) : Tokens.color(.label2))
                        Text(verbatim: option)
                            .font(TypeStyle.body)
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
                    .padding(.horizontal, Tokens.space[3])
                    .padding(.vertical, Tokens.space[2])
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .background(picked ? Tokens.color(.accent) : Tokens.color(.cell),
                                in: RoundedRectangle(cornerRadius: Tokens.radius.block))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!enabled)
                .accessibilityAddTraits(picked ? .isSelected : [])
            }
        }
    }
}

extension View {
    /// Return in a multi-line field sends instead of starting a new line (a pasted text that ends with a line break
    /// sends too). `send` decides whether there is anything to send.
    func sendOnReturn(_ text: Binding<String>, send: @escaping () -> Void) -> some View {
        self
            .submitLabel(.send)
            .onChange(of: text.wrappedValue) { _, new in
                guard new.hasSuffix("\n") else { return }
                text.wrappedValue = String(new.dropLast())
                send()
            }
    }
}
