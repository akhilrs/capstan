import CapstanAppCore
import CapstanKit
import SwiftUI

/// The agents of every project: those that need attention first, then one folded group per project (pinned first,
/// then A to Z). A group opens in place and shows four agents, then "Show N more"; while a filter or a search is on,
/// matching groups are open and show every row. Rows are lazy, so many projects stay cheap.
struct AllAgentsView: View {
    let model: AppModel
    let show: AgentShow
    let query: String
    @Binding var pick: AgentPick?
    let onShow: (AgentShow) -> Void
    let onClearSearch: () -> Void

    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""
    /// Projects opened while showing all agents.
    @State private var expanded: Set<String> = []
    /// Projects closed by the person while a filter or a search is on (those are open by default).
    @State private var closedWhileFiltering: Set<String> = []
    /// Projects whose every agent is shown.
    @State private var showingAll: Set<String> = []

    /// How many agents an open project shows before "Show N more".
    private static let firstAgents = 4

    private enum Entry: Identifiable {
        case title(String, attention: Bool, id: String)
        case attention(AttentionItem)
        case group(AgentGroup, rows: [AgentRow], open: Bool, pinned: Bool)
        case agent(AgentGroup, AgentRow)
        case more(AgentGroup, hidden: Int)

        var id: String {
            switch self {
            case .title(_, _, let id): return "t/" + id
            case .attention(let item): return "a/" + item.id
            case .group(let group, _, _, _): return "g/" + group.id
            case .agent(let group, let row): return "r/" + group.id + "/" + row.source.key
            case .more(let group, _): return "m/" + group.id
            }
        }
    }

    var body: some View {
        let groups = model.agentGroups().sorted(by: AgentShow.byName)
        let all = groups.reduce(0) { $0 + $1.rows.count }
        let needle = query.trimmingCharacters(in: .whitespaces)
        let filtering = !needle.isEmpty || show != .all
        let pinned = ProjectPins.list(pinnedRaw)
        let shown: [(group: AgentGroup, rows: [AgentRow])] = groups.compactMap { group in
            let rows = AgentShow.matching(group, show: show, needle: needle)
            return rows.isEmpty ? nil : (group, rows)
        }
        let pinnedGroups = pinned.compactMap { id in shown.first { $0.group.id == id } }
        let otherGroups = shown.filter { !pinned.contains($0.group.id) }
        let attention = AgentShow.attention(groups)
        let shownCount = shown.reduce(0) { $0 + $1.rows.count }
        let empty = AgentShow.emptiness(all: all, shown: shownCount, show: show, needle: needle)

        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                Text(verbatim: summary(groups, all: all, shown: shownCount, needle: needle, shownGroups: shown.count))
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[0])
                if let empty {
                    emptyState(empty)
                } else {
                    ForEach(entries(
                        attention: needle.isEmpty && show == .all ? attention : [], pinned: pinnedGroups,
                        others: otherGroups, filtering: filtering, groupCount: groups.count)
                    ) { entry in
                        row(entry)
                    }
                }
                // Keeps the last row above the floating status capsule.
                Color.clear.frame(height: StatusCapsule.reservedHeight)
            }
            .padding(.horizontal, Tokens.space[1])
            .padding(.vertical, Tokens.space[1])
        }
    }

    private func summary(_ groups: [AgentGroup], all: Int, shown: Int, needle: String, shownGroups: Int) -> String {
        if !needle.isEmpty && shown > 0 {
            let a = shown == 1 ? "1 agent matches" : String(shown) + " agents match"
            return a + " \"" + needle + "\" in " + (shownGroups == 1 ? "1 project" : String(shownGroups) + " projects")
        }
        return AgentShow.summary(all, projects: groups.count)
    }

    private func entries(
        attention: [AttentionItem], pinned: [(group: AgentGroup, rows: [AgentRow])],
        others: [(group: AgentGroup, rows: [AgentRow])], filtering: Bool, groupCount: Int
    ) -> [Entry] {
        var list: [Entry] = []
        if !attention.isEmpty {
            list.append(.title("Needs attention · " + String(attention.count), attention: true, id: "attention"))
            list += attention.map { Entry.attention($0) }
        }
        func append(_ items: [(group: AgentGroup, rows: [AgentRow])], pinned isPinned: Bool) {
            for item in items {
                let id = item.group.id
                let open = groupCount == 1 || (filtering ? !closedWhileFiltering.contains(id) : expanded.contains(id))
                list.append(.group(item.group, rows: item.rows, open: open, pinned: isPinned))
                guard open else { continue }
                let every = filtering || showingAll.contains(id)
                let rows = every ? item.rows : Array(item.rows.prefix(Self.firstAgents))
                list += rows.map { Entry.agent(item.group, $0) }
                if rows.count < item.rows.count { list.append(.more(item.group, hidden: item.rows.count - rows.count)) }
            }
        }
        if !pinned.isEmpty {
            list.append(.title("Pinned · " + String(pinned.count), attention: false, id: "pinned"))
            append(pinned, pinned: true)
        }
        if !others.isEmpty {
            if !pinned.isEmpty {
                list.append(.title("Other projects · " + String(others.count), attention: false, id: "others"))
            }
            append(others, pinned: false)
        }
        return list
    }

    @ViewBuilder
    private func row(_ entry: Entry) -> some View {
        switch entry {
        case .title(let text, let attention, _):
            ListSectionTitle(text: text, attention: attention)
        case .attention(let item):
            attentionRow(item)
        case .group(let group, let rows, let open, let isPinned):
            groupRow(group, rows: rows, open: open, pinned: isPinned)
        case .agent(let group, let row):
            agentRow(group, row)
        case .more(let group, let hidden):
            Button { showingAll.insert(group.id) } label: {
                Text(verbatim: "Show " + String(hidden) + " more").foregroundStyle(Tokens.color(.accent))
            }
            .buttonStyle(.plain)
            .padding(.leading, Tokens.space[5] + Tokens.space[2])
            .padding(.vertical, Tokens.space[0])
        }
    }

    // MARK: Rows

    private func attentionRow(_ item: AttentionItem) -> some View {
        let row = item.row
        return Button {
            select(AgentPick(hostId: item.group.hostId, projectId: item.group.projectId, agentId: row.source.agentId))
        } label: {
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: row.agentId).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    .frame(width: 132, alignment: .leading)
                Text(verbatim: item.group.projectName + " · " + row.role)
                    .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health)).fixedSize()
            }
            .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[1])
            .listRow(selected: isPicked(item.group, row.source.agentId))
        }
        .buttonStyle(.plain)
    }

    private func groupRow(_ group: AgentGroup, rows: [AgentRow], open: Bool, pinned: Bool) -> some View {
        let counts = HealthCounts(group.rows)
        return Button {
            toggle(group.id, open: open)
            select(AgentPick(hostId: group.hostId, projectId: group.projectId, agentId: nil))
        } label: {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack(spacing: Tokens.space[1]) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold)).frame(width: 12)
                        .foregroundStyle(Tokens.color(.label2))
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .accessibilityHidden(true)
                    Text(verbatim: group.projectName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    if pinned {
                        Image(systemName: "pin.fill").imageScale(.small).foregroundStyle(Tokens.color(.label2))
                            .accessibilityLabel("Pinned")
                    }
                    Spacer(minLength: Tokens.space[1])
                    Text(verbatim: group.hostName + " · " + agentWord(group.rows.count)
                        + (rows.count != group.rows.count ? " · " + String(rows.count) + " shown" : ""))
                        .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                }
                HStack(spacing: Tokens.space[1]) {
                    HealthBar(counts: counts)
                    HStack(spacing: 0) {
                        if !counts.alert.isEmpty {
                            Text(verbatim: counts.alert).fontWeight(.semibold).foregroundStyle(counts.tone.color)
                            if !counts.rest.isEmpty {
                                Text(verbatim: " · " + counts.rest).foregroundStyle(Tokens.color(.label2))
                            }
                        } else {
                            Text(verbatim: counts.rest).foregroundStyle(Tokens.color(.label2))
                        }
                    }
                    .font(.system(size: 12)).lineLimit(1)
                }
                .padding(.leading, 20)
            }
            .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[1])
            .listRow(selected: pick == AgentPick(hostId: group.hostId, projectId: group.projectId, agentId: nil))
        }
        .buttonStyle(.plain)
        .accessibilityValue(open ? "Open" : "Closed")
        .contextMenu {
            Button {
                pinnedRaw = ProjectPins.toggled(group.id, in: pinnedRaw)
            } label: {
                Text(verbatim: (pinned ? "Unpin " : "Pin ") + group.projectName)
            }
            Button {
                model.selection = .agents(hostId: group.hostId, projectId: group.projectId)
            } label: {
                Text(verbatim: "Show only " + group.projectName)
            }
        }
    }

    private func agentRow(_ group: AgentGroup, _ row: AgentRow) -> some View {
        Button {
            select(AgentPick(hostId: group.hostId, projectId: group.projectId, agentId: row.source.agentId))
        } label: {
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: row.agentId).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    .frame(width: 132, alignment: .leading)
                Text(verbatim: row.role).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    .frame(width: 112, alignment: .leading)
                Text(verbatim: row.branch ?? "–").font(.system(size: 12, design: .monospaced)).lineLimit(1)
                    .truncationMode(.middle).frame(maxWidth: .infinity, alignment: .leading)
                StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health)).fixedSize()
            }
            .padding(.leading, Tokens.space[5] + Tokens.space[2]).padding(.trailing, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            .listRow(selected: isPicked(group, row.source.agentId))
        }
        .buttonStyle(.plain)
    }

    private func emptyState(_ empty: AgentEmptiness) -> some View {
        switch empty {
        case .noAgents:
            return ListEmptyState(symbol: "person.2", title: empty.title, detail: empty.detail, actionTitle: "Open Hosts") {
                model.hostsFocus = nil
                model.selection = .hosts
            }
        case .nothingForFilter:
            return ListEmptyState(
                symbol: "line.3.horizontal.decrease.circle", title: empty.title, detail: empty.detail,
                actionTitle: "Show all agents") { onShow(.all) }
        case .noMatch:
            return ListEmptyState(
                symbol: "magnifyingglass", title: empty.title, detail: empty.detail, actionTitle: "Clear search",
                action: onClearSearch)
        }
    }

    // MARK: State

    private func agentWord(_ count: Int) -> String { count == 1 ? "1 agent" : String(count) + " agents" }

    private func isPicked(_ group: AgentGroup, _ agentId: String) -> Bool {
        pick == AgentPick(hostId: group.hostId, projectId: group.projectId, agentId: agentId)
    }

    private func select(_ value: AgentPick) {
        pick = value
        model.selectedAgent = value.agentId
    }

    private func toggle(_ id: String, open: Bool) {
        if show != .all || !query.trimmingCharacters(in: .whitespaces).isEmpty {
            if open { closedWhileFiltering.insert(id) } else { closedWhileFiltering.remove(id) }
        } else if open {
            expanded.remove(id)
        } else {
            expanded.insert(id)
        }
    }
}

/// The inspector for a project row of the all-projects Agents list.
struct ProjectGroupInspector: View {
    let model: AppModel
    let hostId: UUID
    let projectId: String
    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""

    var body: some View {
        if let group = ListData.agentGroups(model, scope: ProjectKey(hostId: hostId, projectId: projectId)).first {
            let counts = HealthCounts(group.rows)
            let pinned = ProjectPins.isPinned(group.id, in: pinnedRaw)
            let project = model.project(hostId, projectId)
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                ScrollView {
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        Text(verbatim: "Project · " + group.hostName)
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                        Text(verbatim: group.projectName).font(.system(size: 20, weight: .semibold))
                        Text(verbatim: (counts.alert.isEmpty ? "" : counts.alert + " · ") + counts.rest)
                            .font(.system(size: 13))
                        MetaList(rows: [
                            MetaRow(label: "Agents", value: plain(
                                String(counts.total) + (counts.alert.isEmpty ? " · none needs attention" : ""))),
                            MetaRow(label: "Run", value: plain((project?.run?.paused ?? false) ? "paused" : "running")),
                            MetaRow(label: "Host", value: plain(group.hostName)),
                        ])
                    }
                }
                HStack {
                    Spacer()
                    Button { pinnedRaw = ProjectPins.toggled(group.id, in: pinnedRaw) } label: {
                        Text(verbatim: pinned ? "Unpin" : "Pin")
                    }
                    .buttonStyle(.capstanQuiet)
                    Button { model.selection = .agents(hostId: hostId, projectId: projectId) } label: {
                        Text(verbatim: "Show only " + group.projectName)
                    }
                    .buttonStyle(.capstanPrimary)
                }
            }
            .padding(Tokens.space[3])
        } else {
            Text("Select an agent to see what it is doing")
                .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func plain(_ s: String) -> UntrustedText {
        UntrustedText(raw: s, verified: true, truncated: false)
    }
}
