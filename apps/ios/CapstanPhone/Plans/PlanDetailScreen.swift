import CapstanAppCore
import CapstanKit
import SwiftUI

/// A plan's details (board 08): its bar and the counts in words, then its packages in integration order with the
/// branch, the state and review, and the order (dependencies). A host without package details says so; a plan
/// with integration items waiting links to them.
struct PlanDetailScreen: View {
    @Environment(AppModel.self) private var model
    let ref: PlanRef

    var body: some View {
        if let project = model.project(ref.hostId, ref.projectId),
            let row = PlanRow.rows(project).first(where: { $0.planId == ref.planId })
        {
            content(row, project)
        } else {
            ContentUnavailableView("This plan is no longer listed", systemImage: "list.bullet.rectangle")
                .background(Tokens.color(.ground))
        }
    }

    private func content(_ row: PlanRow, _ project: ProjectModel) -> some View {
        let stored = PlanRow.plan(for: row, in: project)
        let packages = stored.map { PackageRow.rows($0, in: project) } ?? []
        let waiting = IntegrationLink.waiting(for: stored, in: project)
        let projectName = TextBudget.displayPrefix(project.displayName, maxBytes: 64).text
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: "list.bullet.rectangle").foregroundStyle(Tokens.color(.accent)).accessibilityHidden(true)
                        Text(verbatim: "Plan · " + projectName + " · " + row.planId)
                            .font(TypeStyle.subheadline)
                            .foregroundStyle(Tokens.color(.label2))
                    }
                    Text(verbatim: row.title).font(TypeStyle.title2).accessibilityAddTraits(.isHeader)
                }
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    PlanProgressBar(progress: row.progress).accessibilityHidden(true)
                    Text(verbatim: row.stateWord + " · " + row.words)
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                }
                ForEach(waiting, id: \.key) { item in
                    Button {
                        let tab = model.navigation.tab
                        model.navigation.show(
                            tab,
                            model.navigation.path(tab)
                                + [.item(InboxRowID(hostId: ref.hostId, projectId: ref.projectId, key: item.key))])
                    } label: {
                        DetailNotice(
                            tone: .yellow, symbol: "arrow.triangle.merge", title: "Waits for you",
                            text: RowText.inspectorTitle(kind: item.kind, key: item.key, summary: item.summary.displayPlain))
                    }
                    .buttonStyle(.plain)
                }
                if packages.isEmpty {
                    if row.total == 0 {
                        Text("This plan has no packages yet.")
                            .font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                    } else {
                        DetailNotice(
                            tone: .grey, symbol: "info.circle", title: "No package details from this host",
                            text: "Update Capstan on the host to see each package's branch, review and order.")
                    }
                } else {
                    Text("Packages, in integration order").font(TypeStyle.headline)
                    VStack(spacing: 0) {
                        ForEach(Array(packages.enumerated()), id: \.element.id) { index, package in
                            if index > 0 { Divider().padding(.leading, Tokens.space[6]) }
                            PackageCell(number: index + 1, package: package)
                        }
                        if row.omittedPackages > 0 {
                            Divider()
                            Text(verbatim: "+" + String(row.omittedPackages) + " more packages on the host")
                                .font(TypeStyle.footnote)
                                .foregroundStyle(Tokens.color(.label2))
                                .padding(Tokens.space[3])
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// One package: its place in the order, its id and title, the branch, the state and review pills, and the order.
struct PackageCell: View {
    let number: Int
    let package: PackageRow

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[3]) {
            Text(verbatim: String(number))
                .font(TypeStyle.subheadline.monospacedDigit())
                .foregroundStyle(Tokens.color(.label2))
                .frame(minWidth: 16, alignment: .trailing)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text(verbatim: package.id).font(TypeStyle.headline)
                if package.title != package.id {
                    Text(verbatim: package.title).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                }
                Text(verbatim: package.branch ?? "–")
                    .font(TypeStyle.monoSmall)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                    .truncationMode(.middle)
                HStack(spacing: Tokens.space[1]) {
                    StatePill(word: package.pillWord, symbol: package.stateSymbol, tone: package.pillTone)
                    if package.reviewWord != "Not reviewed" {
                        StatePill(
                            word: "Review " + package.reviewWord.lowercased(),
                            symbol: package.reviewWord == "Passed" ? "checkmark" : "arrow.clockwise",
                            tone: package.reviewWord == "Passed" ? .green : .yellow)
                    }
                    if let integration = package.integrationWord {
                        StatePill(word: integration, symbol: nil, tone: integration == "Conflicted" ? .yellow : .blue)
                    }
                }
                Text(verbatim: package.detailLine).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[3])
        .accessibilityElement(children: .combine)
    }
}

#Preview("Plans") {
    NavigationStack { PlansScreen().routes() }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Plan detail") {
    NavigationStack { PlanDetailScreen(ref: PlanRef(hostId: PreviewData.alphaId, projectId: "capstan", planId: "p-reported")) }
        .environment(AppModel(preview: PreviewData.standard))
}
