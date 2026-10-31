import CapstanAppCore
import CapstanKit
import SwiftUI

struct MetaRow {
    let label: String
    let value: UntrustedText
}

/// Label and value rows of a detail screen; values are host text and shown as plain text. The label sits above the
/// value, so long values wrap at every Dynamic Type size.
struct MetaList: View {
    let rows: [MetaRow]

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: row.label).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    if row.value.verified {
                        Text(verbatim: TextBudget.displayPrefix(row.value.displayPlain, maxBytes: ProtocolLimits.textCap).text)
                            .font(TypeStyle.body)
                            .textSelection(.enabled)
                    } else {
                        Text("Not shown: the host could not vouch for it")
                            .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
    }
}

#Preview("MetaList") {
    MetaList(rows: [
        MetaRow(label: "Branch", value: UntrustedText(raw: "feat/plan-92", verified: true, truncated: false)),
        MetaRow(label: "Commit", value: UntrustedText(raw: "", verified: false, truncated: false)),
    ])
    .padding()
}
