import CapstanAppCore
import CapstanKit
import SwiftUI

/// The packages of one plan in integration order: id and title, state, review, what it depends on, who has it and
/// its integration state. A plan from a host without details shows nothing here.
struct PackageRowsView: View {
    let plan: PlanRow
    let packages: [PackageRow]

    var body: some View {
        LazyVStack(alignment: .leading, spacing: Tokens.space[1]) {
            ForEach(packages, id: \.id) { package in
                packageRow(package)
            }
            if plan.omittedPackages > 0 {
                Text(verbatim: "+\(plan.omittedPackages) more packages")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .padding(.leading, Tokens.space[2])
            }
        }
        .padding(.leading, Tokens.space[3])
    }

    private func packageRow(_ package: PackageRow) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack {
                Text(verbatim: package.id).font(.system(size: 12, design: .monospaced))
                Text(verbatim: package.title).font(.system(size: 13)).lineLimit(1)
                Spacer()
                StatePill(word: package.stateWord, symbol: package.stateSymbol, tone: tone(package))
            }
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: package.reviewWord)
                if !package.dependsOn.isEmpty {
                    Text(verbatim: "depends on").foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: package.dependsOn.joined(separator: ", ")).font(.system(size: 12, design: .monospaced))
                }
                if let assignee = package.assignee {
                    Text(verbatim: assignee).font(.system(size: 12, design: .monospaced))
                }
                if let integration = package.integrationWord {
                    Text(verbatim: integration)
                }
            }
            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        }
        .padding(Tokens.space[1])
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private func tone(_ package: PackageRow) -> Tone {
        switch package.stateWord {
        case "Integrated", "Confirmed": return .green
        case "Reported": return .blue
        case "Assigned": return .yellow
        default: return .grey
        }
    }
}

#if DEBUG
private func sampleDetail(
    _ id: String, _ title: String, state: PackageState, review: String? = nil, dependsOn: [String] = [],
    assignee: String? = nil, integration: String? = nil, verified: Bool = true
) -> PackageDetail {
    PackageDetail(
        id: id, title: UntrustedText(raw: title, verified: verified, truncated: false), role: "developer",
        assigneeAgentId: assignee, state: state, dependsOn: dependsOn, review: review, integrationState: integration)
}

private func samplePlan(_ details: [PackageDetail]?, omitted: Int64?) -> StoredPlan {
    StoredPlan(
        key: "plan:plan-1", planId: "plan-1", sequence: 1,
        title: UntrustedText(raw: "Plans screen", verified: true, truncated: false), state: "active",
        packages: StoredCounts(total: 4 + (omitted ?? 0), assigned: 3, reported: 2, integrated: 1),
        packageDetails: details, packageDetailsOmitted: omitted)
}

private let sampleDetails = [
    sampleDetail("kit-details", "Kit details", state: .integrated, review: "passed", integration: "merged"),
    sampleDetail("ui-plans", "Plans rows", state: .reported, review: "findings", dependsOn: ["kit-details"], assignee: "developer-355"),
    sampleDetail("ui-agents", "Agents column", state: .assigned, dependsOn: ["kit-details"], assignee: "developer-356"),
    sampleDetail("docs", "Hostile title", state: .unassigned, dependsOn: ["ui-plans", "ui-agents"], verified: false),
]

#Preview("Packages with details") {
    let plan = samplePlan(sampleDetails, omitted: nil)
    PackageRowsView(plan: PlanRow(plan), packages: PackageRow.rows(plan)).frame(width: 520).padding()
}

#Preview("Packages without details") {
    let plan = samplePlan(nil, omitted: nil)
    PackageRowsView(plan: PlanRow(plan), packages: PackageRow.rows(plan)).frame(width: 520).padding()
}

#Preview("Packages with omitted rows") {
    let plan = samplePlan(sampleDetails, omitted: 60)
    PackageRowsView(plan: PlanRow(plan), packages: PackageRow.rows(plan)).frame(width: 520).padding()
}
#endif
