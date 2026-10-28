import CapstanAppCore
import CapstanKit
import SwiftUI

/// The plans of a project with a progress bar and the counts in words.
struct PlansScreen: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String

    private var project: ProjectModel? { model.project(hostId, projectId) }

    var body: some View {
        if let project {
            let rows = PlanRow.rows(project)
            let waiting = IntegrationLink.waitingCount(project)
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    if rows.isEmpty {
                        Text("No plans yet").foregroundStyle(Tokens.color(.label2))
                    }
                    ForEach(rows, id: \.planId) { row in
                        planRow(row, waiting: waiting)
                    }
                }
                .padding(Tokens.space[4])
            }
        } else {
            Text("This project is not available").foregroundStyle(Tokens.color(.label2)).padding()
        }
    }

    private func planRow(_ row: PlanRow, waiting: Int) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: row.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
            ProgressBar(progress: row.progress)
            Text(verbatim: row.words).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            if row.hasUnintegratedWork, waiting > 0 {
                Button {
                    model.showProject(hostId: hostId, projectId: projectId, kind: .integration)
                } label: {
                    Text(verbatim: waiting == 1 ? "1 integration waiting" : "\(waiting) integrations waiting")
                }
                .buttonStyle(.link)
                .controlSize(.small)
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.row))
        .accessibilityElement(children: .combine)
    }
}

/// Four stages in one bar: integrated, reported, assigned, not started.
private struct ProgressBar: View {
    let progress: PlanProgress

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: 0) {
                segment(progress.integrated, Tokens.color(.green), geo.size.width)
                segment(progress.reported, Tokens.color(.blue), geo.size.width)
                segment(progress.assigned, Tokens.color(.yellow), geo.size.width)
                Spacer(minLength: 0)
            }
        }
        .frame(height: 6)
        .background(Tokens.color(.fill2), in: Capsule())
        .clipShape(Capsule())
    }

    private func segment(_ share: Double, _ color: Color, _ width: CGFloat) -> some View {
        Rectangle().fill(color).frame(width: width * share)
    }
}

#Preview("Plans: several progress points") {
    PlansScreen(model: AppModel(preview: PreviewData.standard), hostId: PreviewData.alphaId, projectId: "capstan")
        .frame(width: 520, height: 420)
}
