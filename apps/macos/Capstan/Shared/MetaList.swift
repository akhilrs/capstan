import CapstanAppCore
import CapstanKit
import SwiftUI

struct MetaRow {
    let label: String
    let value: UntrustedText
}

/// Label and value rows of an inspector; values are host text and shown as plain text.
struct MetaList: View {
    let rows: [MetaRow]

    var body: some View {
        Grid(alignment: .topLeading, horizontalSpacing: Tokens.space[2], verticalSpacing: Tokens.space[1]) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                GridRow {
                    Text(verbatim: row.label).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    if row.value.verified {
                        Text(verbatim: TextBudget.displayPrefix(row.value.displayPlain, maxBytes: ProtocolLimits.textCap).text)
                            .font(.system(size: 13))
                            .textSelection(.enabled)
                    } else {
                        Text("Not shown: the host could not vouch for it")
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                }
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
