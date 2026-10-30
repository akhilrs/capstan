import CapstanAppCore
import CapstanKit
import SwiftUI

/// Hosts: per host a table of its projects with the state of each, what it means and how many items wait.
struct HostsScreen: View {
    let model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[4]) {
                if model.hosts.isEmpty {
                    Text("No hosts yet. Add one to see its projects.")
                        .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                }
                ForEach(model.hosts) { host in
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        hostHeader(host)
                        ConnectionBanners(model: model, hostId: host.id)
                        table(host)
                    }
                }
                // Keeps the last row above the floating status capsule.
                Color.clear.frame(height: StatusCapsule.reservedHeight)
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func hostHeader(_ host: HostConfig) -> some View {
        let list = HostLabel.projects(model.hostModels[host.id])
        let status = SidebarText.hostStatus(state: model.connection[host.id], projects: list)
        var line = status.word
        if let version = model.hostModels[host.id]?.hello?.version, version.verified {
            line += " · Capstan " + TextBudget.displayPrefix(version.displayPlain, maxBytes: 32).text
        }
        line += " · \(list.count) project" + (list.count == 1 ? "" : "s")
        // Not a button: the header is not selectable, so no focus or selection ring can surround it.
        return HStack(spacing: Tokens.space[1]) {
            Circle()
                .fill(status.tone == .green ? status.tone.color : Color.clear)
                .overlay(Circle().strokeBorder(status.tone.color, lineWidth: status.tone == .green ? 0 : 1.5))
                .frame(width: 8, height: 8)
            Text(verbatim: HostLabel.hostName(host)).font(.system(size: 15, weight: .semibold))
            Text(verbatim: line).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }

    private func table(_ host: HostConfig) -> some View {
        let list = HostLabel.projects(model.hostModels[host.id])
        return VStack(spacing: 0) {
            HStack(spacing: Tokens.space[2]) {
                head("Project", width: 140)
                head("State", width: 110)
                head("What it means", width: nil)
                head("Wait", width: 40, trailing: true)
            }
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            if list.isEmpty {
                Divider()
                Text("No projects read yet.").font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .padding(Tokens.space[2]).frame(maxWidth: .infinity, alignment: .leading)
            }
            ForEach(list, id: \.projectId) { project in
                Divider()
                projectRow(host, project)
            }
        }
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
    }

    private func head(_ title: String, width: CGFloat?, trailing: Bool = false) -> some View {
        Text(verbatim: title)
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Tokens.color(.label2))
            .frame(width: width, alignment: trailing ? .trailing : .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
    }

    private func projectRow(_ host: HostConfig, _ project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        let focus = HostsFocus(hostId: host.id, projectId: project.projectId)
        let waiting = project.availability == .ready ? project.waiting.count : 0
        return Button {
            model.hostsFocus = focus
        } label: {
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: HostLabel.projectName(project)).font(.system(size: 13)).lineLimit(1)
                    .frame(width: 140, alignment: .leading)
                StatePill(word: text.word, symbol: text.symbol, tone: HostLabel.tone(project.availability))
                    .frame(width: 110, alignment: .leading)
                Text(verbatim: waitMeaning(project, text)).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(verbatim: waiting > 0 ? String(waiting) : "–").font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(Tokens.color(.label2))
                    .frame(width: 40, alignment: .trailing)
            }
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            .background(model.hostsFocus == focus ? Tokens.color(.accent).opacity(0.16) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            if ProjectRemovalPrompt.isOffered(project.availability) {
                Button(ProjectRemovalPrompt.menuTitle) { // static-text
                    model.projectToRemove = ProjectKey(hostId: host.id, projectId: project.projectId)
                }
            }
        }
    }

    private func waitMeaning(_ project: ProjectModel, _ text: AvailabilityText) -> String {
        project.availability == .ready ? "Following live" : text.meaning
    }
}

#Preview("Hosts screen") {
    HostsScreen(model: AppModel(preview: PreviewData.standard)).frame(width: 760, height: 620)
}

#Preview("Hosts screen: stopped") {
    HostsScreen(model: AppModel(preview: HostsPreviewFixtures.seed(
        connection: .stopped(.cstanMissing(tail: UntrustedText(raw: "not found", verified: true, truncated: false))))))
        .frame(width: 760, height: 620)
}
