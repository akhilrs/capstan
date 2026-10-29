import CapstanAppCore
import CapstanKit
import SwiftUI

/// The packages of one plan in integration order, as the design draws them: a status icon, the id and a coloured
/// pill, then the branch in monospace and the review state and order. A plan from a host without details shows nothing.
struct PackageRowsView: View {
    let plan: PlanRow
    let packages: [PackageRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(packages, id: \.id) { package in
                packageRow(package)
                Divider()
            }
            if plan.omittedPackages > 0 {
                Text(verbatim: "+\(plan.omittedPackages) more packages")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .padding(.top, Tokens.space[1])
            }
        }
    }

    /// Green for integrated, blue for reported, the full text colour for the outlined and dashed circles.
    private func iconColor(_ package: PackageRow) -> Color {
        switch package.pillTone {
        case .green: return Tokens.color(.green)
        case .blue: return Tokens.color(.blue)
        default: return Tokens.color(.label)
        }
    }

    private func packageRow(_ package: PackageRow) -> some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            Image(systemName: package.statusIcon)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(iconColor(package))
                .frame(width: 16, height: 16)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack {
                    Text(verbatim: package.id).font(.system(size: 13, weight: .semibold))
                    Spacer()
                    StatePill(word: package.pillWord, symbol: nil, tone: package.pillTone)
                }
                if let branch = package.branch {
                    Text(verbatim: branch).font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Tokens.color(.label2)).lineLimit(1).truncationMode(.middle)
                }
                Text(verbatim: package.detailLine)
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(.vertical, Tokens.space[1])
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
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
