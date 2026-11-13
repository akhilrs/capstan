import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Workspace overview (board 07): the repos of the project in merge order and the worktrees the agents hold. It
/// shows what the feed carries; a host that sends no workspace data gets the older-host note.
struct WorkspaceScreen: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String

    private var project: ProjectModel? { model.project(hostId, projectId) }
    private var hostName: String {
        model.hosts.first { $0.id == hostId }.map(HostLabel.hostName) ?? "the host"
    }

    var body: some View {
        if let project {
            let overview = WorkspaceOverview.make(project)
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[3]) {
                    if project.availability != .ready, overview.isEmpty {
                        note(WorkspaceOverview.loading(host: hostName))
                    } else if overview.isEmpty {
                        note(WorkspaceOverview.older)
                    } else {
                        repos(overview)
                        worktrees(overview)
                        cleanups(overview)
                    }
                    Color.clear.frame(height: StatusCapsule.reservedHeight)
                }
                .padding(Tokens.space[3])
            }
        } else {
            note(WorkspaceOverview.loading(host: hostName)).padding(Tokens.space[3])
        }
    }

    private func note(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func repos(_ overview: WorkspaceOverview) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: overview.reposCount).font(.system(size: 13, weight: .semibold))
            ForEach(overview.repos) { row in
                HStack(spacing: Tokens.space[2]) {
                    Text(verbatim: row.position.map(String.init) ?? "–")
                        .font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                        .frame(width: 20, alignment: .trailing)
                    Text(verbatim: row.repo)
                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                        .lineLimit(1).truncationMode(.middle)
                    Spacer()
                    if let main = row.mainBranch {
                        Text(verbatim: WorkspaceOverview.mainBranchLabel + " " + main)
                            .font(.system(size: 12, design: .monospaced)).foregroundStyle(Tokens.color(.label2))
                            .lineLimit(1).truncationMode(.middle)
                    }
                    Text(verbatim: "Worktrees \(row.worktrees)").font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: "Open sets \(row.openSets)").font(.system(size: 12).monospacedDigit())
                        .foregroundStyle(Tokens.color(.label2))
                }
                .accessibilityElement(children: .combine)
                Divider()
            }
        }
    }

    private func worktrees(_ overview: WorkspaceOverview) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: WorkspaceOverview.worktreesHeading).font(.system(size: 13, weight: .semibold))
            if overview.agents.isEmpty {
                note(WorkspaceOverview.noWorktrees)
            }
            ForEach(overview.agents) { group in
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: group.heading).font(.system(size: 12, weight: .semibold))
                    ForEach(Array(group.worktrees.enumerated()), id: \.offset) { _, tree in
                        HStack(spacing: Tokens.space[1]) {
                            Text(verbatim: tree.repo).font(.system(size: 12, design: .monospaced))
                            if let branch = tree.branch {
                                Text(verbatim: branch).font(.system(size: 12, design: .monospaced))
                                    .foregroundStyle(Tokens.color(.label2))
                            }
                            Text(verbatim: tree.path).font(.system(size: 12, design: .monospaced))
                                .foregroundStyle(Tokens.color(.label2)).lineLimit(1).truncationMode(.middle)
                                .textSelection(.enabled)
                        }
                    }
                    if let folder = group.changeFolder {
                        Text(verbatim: WorkspaceOverview.changeFolderLabel + " " + folder)
                            .font(.system(size: 12, design: .monospaced)).foregroundStyle(Tokens.color(.label2))
                            .textSelection(.enabled)
                    }
                }
                .padding(Tokens.space[2])
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            }
        }
    }

    private func cleanups(_ overview: WorkspaceOverview) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: WorkspaceOverview.cleanupsHeading).font(.system(size: 13, weight: .semibold))
            if overview.cleanups.isEmpty {
                note(WorkspaceOverview.noCleanups)
            }
            ForEach(overview.cleanups) { row in
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: row.line).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                    if let note = row.leftNote {
                        Text(verbatim: note).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    }
                    if let command = row.applyCommand {
                        Text(verbatim: command).font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(Tokens.color(.label2)).textSelection(.enabled)
                    }
                }
                .padding(Tokens.space[2])
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
            }
        }
    }
}
