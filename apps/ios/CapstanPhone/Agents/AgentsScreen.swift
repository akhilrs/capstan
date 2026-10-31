import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Agents tab, redesigned for many projects: the agents that need attention first, in one card across every
/// project; a filter (All, Attention, Working, Paused); then one collapsed row per project with its host, its agent
/// count, a bar and its counts in words. Pinned projects come first, the others follow A to Z. A project opens in
/// place and shows four agents, then "Show N more". Search matches agent ids, roles, project and host names. The
/// jump sheet lists pinned, recent and all projects and scrolls to the one chosen.
struct AgentsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var show: AgentShow = .all
    @State private var query = ""
    /// Projects opened while showing all agents.
    @State private var expanded: Set<String> = []
    /// Projects closed while a filter or a search is on (those open by default).
    @State private var closedWhileFiltering: Set<String> = []
    /// Projects whose every agent is shown.
    @State private var showingAll: Set<String> = []
    @State private var jumping = false
    @State private var jumpTarget: String?
    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""
    @AppStorage(ProjectPins.recentKey) private var recentRaw = ""

    /// How many agents an open project shows before "Show N more".
    private static let firstAgents = 4

    var body: some View {
        let groups = model.agentGroups().sorted(by: AgentsScreen.byName)
        let all = groups.flatMap { $0.rows }
        let needle = query.trimmingCharacters(in: .whitespaces)
        let filtering = !needle.isEmpty || show != .all
        let pinned = ProjectPins.list(pinnedRaw)
        let shown: [ShownGroup] = groups.compactMap { group in
            let rows = AgentsScreen.matching(group, show: show, needle: needle)
            return rows.isEmpty ? nil : ShownGroup(group: group, rows: rows)
        }
        let pinnedGroups = pinned.compactMap { id in shown.first { $0.group.id == id } }
        let otherGroups = shown.filter { !pinned.contains($0.group.id) }
        let attention = AgentsScreen.attention(groups)

        ScrollViewReader { proxy in
            List {
                Section {
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        Text(verbatim: AgentsScreen.summary(all.count, projects: groups.count))
                            .font(TypeStyle.subheadline)
                            .foregroundStyle(Tokens.color(.label2))
                        Picker(selection: $show) {
                            ForEach(AgentShow.allCases, id: \.self) { option in
                                Text(verbatim: option.title + " " + String(all.filter(option.matches).count)).tag(option)
                            }
                        } label: {
                            Text("Show")
                        }
                        .pickerStyle(.segmented)
                    }
                    .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                }

                if needle.isEmpty && show == .all && !attention.isEmpty {
                    Section {
                        ForEach(attention) { item in
                            NavigationLink(value: PhoneRoute.agent(
                                hostId: item.group.hostId, projectId: item.group.projectId, agentId: item.row.source.agentId)
                            ) {
                                AttentionAgentRow(row: item.row, projectName: item.group.projectName)
                            }
                        }
                    } header: {
                        Text(verbatim: "Needs attention · " + String(attention.count))
                            .foregroundStyle(Tokens.color(.accent))
                    }
                }

                if all.isEmpty {
                    Section {
                        ContentUnavailableView(
                            "No agents", systemImage: "person.2",
                            description: Text("The agents of your ready projects appear here."))
                    }
                    .listRowBackground(Color.clear)
                } else if shown.isEmpty {
                    Section {
                        if needle.isEmpty {
                            ContentUnavailableView(
                                "Nothing to show", systemImage: "line.3.horizontal.decrease.circle",
                                description: Text("No agent matches this filter."))
                        } else {
                            ContentUnavailableView.search(text: needle)
                        }
                    }
                    .listRowBackground(Color.clear)
                }

                ForEach(Array(pinnedGroups.enumerated()), id: \.element.group.id) { index, item in
                    projectSection(item, filtering: filtering, single: groups.count == 1, pinned: true,
                                   header: index == 0 ? "Pinned · " + String(pinnedGroups.count) : nil)
                }
                ForEach(Array(otherGroups.enumerated()), id: \.element.group.id) { index, item in
                    projectSection(item, filtering: filtering, single: groups.count == 1, pinned: false,
                                   header: index == 0 && !pinnedGroups.isEmpty
                                       ? "Other projects · " + String(otherGroups.count) : nil)
                }
            }
            .listStyle(.insetGrouped)
            .listSectionSpacing(Tokens.space[1])
            .scrollContentBackground(.hidden)
            .background(Tokens.color(.ground))
            .navigationTitle("Agents")
            .navigationBarTitleDisplayMode(.large)
            .searchable(text: $query, prompt: Text("Agents, roles, projects"))
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        jumping = true
                    } label: {
                        Label("Jump to project", systemImage: "list.bullet")
                    }
                    .disabled(groups.isEmpty)
                }
            }
            .onChange(of: show) { _, _ in Haptics.play(.selection) }
            .sheet(isPresented: $jumping, onDismiss: {
                guard let id = jumpTarget else { return }
                jumpTarget = nil
                withAnimation { proxy.scrollTo(id, anchor: .top) }
            }) {
                ProjectJumpSheet(
                    entries: groups.map(AgentsScreen.jumpEntry),
                    pinned: pinned,
                    recent: ProjectPins.list(recentRaw),
                    togglePin: { pinnedRaw = ProjectPins.toggled($0, in: pinnedRaw) },
                    pick: { id in
                        show = .all
                        query = ""
                        expanded.insert(id)
                        recentRaw = ProjectPins.touched(id, in: recentRaw)
                        jumpTarget = id
                        jumping = false
                    })
            }
        }
    }

    /// One project: its summary row (a button that opens or closes it), then its agents when open.
    @ViewBuilder
    private func projectSection(_ item: ShownGroup, filtering: Bool, single: Bool, pinned: Bool, header: String?) -> some View {
        let id = item.group.id
        let open = single || (filtering ? !closedWhileFiltering.contains(id) : expanded.contains(id))
        let everyRow = filtering || showingAll.contains(id)
        let rows = everyRow ? item.rows : Array(item.rows.prefix(AgentsScreen.firstAgents))
        Section {
            Button {
                toggle(id, filtering: filtering, open: open)
            } label: {
                ProjectAgentsRow(group: item.group, open: open, pinned: pinned)
            }
            .id(id)
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(open ? Text("Open") : Text("Closed"))
            .contextMenu {
                if pinned {
                    Button("Unpin", systemImage: "pin.slash") { pinnedRaw = ProjectPins.toggled(id, in: pinnedRaw) }
                } else {
                    Button("Pin", systemImage: "pin") { pinnedRaw = ProjectPins.toggled(id, in: pinnedRaw) }
                }
            }
            if open {
                ForEach(rows, id: \.source.key) { row in
                    NavigationLink(
                        value: PhoneRoute.agent(hostId: item.group.hostId, projectId: item.group.projectId, agentId: row.source.agentId)
                    ) {
                        AgentListRow(row: row)
                    }
                }
                if rows.count < item.rows.count {
                    Button {
                        showingAll.insert(id)
                    } label: {
                        Text(verbatim: "Show " + String(item.rows.count - rows.count) + " more")
                            .foregroundStyle(Tokens.color(.accent))
                    }
                }
            }
        } header: {
            if let header {
                Text(verbatim: header)
            }
        }
    }

    private func toggle(_ id: String, filtering: Bool, open: Bool) {
        Haptics.play(.selection)
        withAnimation(.easeInOut(duration: Motion.state)) {
            if filtering {
                if open { closedWhileFiltering.insert(id) } else { closedWhileFiltering.remove(id) }
            } else if open {
                expanded.remove(id)
            } else {
                expanded.insert(id)
                recentRaw = ProjectPins.touched(id, in: recentRaw)
            }
        }
    }

    /// A project's agents that pass the filter and the search, those that need attention first. A search that
    /// matches the project or host name keeps every agent of it that passes the filter.
    static func matching(_ group: AgentGroup, show: AgentShow, needle: String) -> [AgentRow] {
        let filtered = group.rows.filter(show.matches)
        let rows: [AgentRow]
        if needle.isEmpty
            || group.projectName.localizedCaseInsensitiveContains(needle)
            || group.hostName.localizedCaseInsensitiveContains(needle) {
            rows = filtered
        } else {
            rows = filtered.filter {
                $0.agentId.localizedCaseInsensitiveContains(needle) || $0.role.localizedCaseInsensitiveContains(needle)
            }
        }
        return rows.sorted { AgentShow.rank($0.health) < AgentShow.rank($1.health) }
    }

    /// Every agent that needs attention, in every project: needs you first, then the rest.
    static func attention(_ groups: [AgentGroup]) -> [AttentionItem] {
        groups.flatMap { group in group.rows.filter(AgentShow.attention.matches).map { AttentionItem(group: group, row: $0) } }
            .sorted { AgentShow.rank($0.row.health) < AgentShow.rank($1.row.health) }
    }

    static func byName(_ left: AgentGroup, _ right: AgentGroup) -> Bool {
        if left.projectName != right.projectName {
            return left.projectName.localizedStandardCompare(right.projectName) == .orderedAscending
        }
        return left.hostName.localizedStandardCompare(right.hostName) == .orderedAscending
    }

    static func jumpEntry(_ group: AgentGroup) -> JumpEntry {
        let counts = HealthCounts(group.rows)
        return JumpEntry(
            id: group.id, name: group.projectName, hostName: group.hostName,
            count: group.rows.count == 1 ? "1 agent" : String(group.rows.count) + " agents",
            state: counts.headline, tone: counts.tone)
    }

    static func summary(_ agents: Int, projects: Int) -> String {
        let a = agents == 1 ? "1 agent" : String(agents) + " agents"
        let p = projects == 1 ? "1 project" : String(projects) + " projects"
        return a + " · " + p
    }
}

/// An agent that needs attention, with its project.
struct AttentionItem: Identifiable {
    let group: AgentGroup
    let row: AgentRow
    var id: String { group.id + "/" + row.source.key }
}

/// A project and the agents of it the screen shows.
private struct ShownGroup {
    let group: AgentGroup
    let rows: [AgentRow]
}

/// What the Agents tab shows: every agent, those that need attention (needs you, stalled, lost, blocked), those
/// working, or those paused.
enum AgentShow: CaseIterable, Hashable {
    case all, attention, working, paused

    var title: String {
        switch self {
        case .all: return "All"
        case .attention: return "Attention"
        case .working: return "Working"
        case .paused: return "Paused"
        }
    }

    func matches(_ row: AgentRow) -> Bool {
        switch self {
        case .all: return true
        case .attention: return AgentFilter.needAttention.matches(row)
        case .working: return row.health == .working
        case .paused: return row.health == .paused
        }
    }

    /// The order inside a project: needs you, then stalled, lost or blocked, then working, paused and idle.
    static func rank(_ health: AgentHealth) -> Int {
        switch health {
        case .needsYou: return 0
        case .stalled, .lost, .blocked: return 1
        case .working: return 2
        case .paused: return 3
        case .idle: return 4
        }
    }
}

/// A project's agents counted by health, in words and as a bar.
struct HealthCounts {
    private var counts: [AgentHealth: Int] = [:]
    let total: Int

    init(_ rows: [AgentRow]) {
        total = rows.count
        for row in rows { counts[row.health, default: 0] += 1 }
    }

    func count(_ health: AgentHealth) -> Int { counts[health] ?? 0 }

    /// "1 needs you · 1 lost": what needs attention, empty when nothing does.
    var alert: String {
        [AgentHealth.needsYou, .stalled, .lost, .blocked].compactMap { health in
            count(health) > 0 ? String(count(health)) + " " + health.word.lowercased() : nil
        }.joined(separator: " · ")
    }

    /// "2 working · 4 idle", or "all idle".
    var rest: String {
        var parts: [String] = []
        if count(.working) > 0 { parts.append(String(count(.working)) + " working") }
        if count(.paused) > 0 { parts.append(String(count(.paused)) + " paused") }
        if count(.idle) > 0 && count(.idle) == total {
            parts.append("all idle")
        } else if count(.idle) > 0 {
            parts.append(String(count(.idle)) + " idle")
        }
        return parts.joined(separator: " · ")
    }

    /// The tone of the most urgent health.
    var tone: Tone {
        if count(.needsYou) > 0 { return .accent }
        if count(.stalled) + count(.lost) + count(.blocked) > 0 { return .red }
        if count(.working) > 0 { return .green }
        return .grey
    }

    /// One short word group for the jump sheet: the most urgent count.
    var headline: String {
        if !alert.isEmpty { return alert.components(separatedBy: " · ").first ?? alert }
        if count(.working) > 0 { return String(count(.working)) + " working" }
        if count(.paused) == total { return "All paused" }
        return "All idle"
    }

    var bar: [(count: Int, color: Color)] {
        [
            (count(.needsYou), Tokens.color(.accent)),
            (count(.stalled) + count(.lost) + count(.blocked), Tokens.color(.red)),
            (count(.working), Tokens.color(.green)),
            (count(.paused), Tokens.color(.blue)),
            (count(.idle), Tokens.color(.label2).opacity(0.35)),
        ]
    }
}

/// The summary row of a project: a chevron, its name, host and agent count, then a bar and its counts in words.
struct ProjectAgentsRow: View {
    let group: AgentGroup
    let open: Bool
    let pinned: Bool

    var body: some View {
        let counts = HealthCounts(group.rows)
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: "chevron.right")
                    .font(TypeStyle.footnote.weight(.semibold))
                    .foregroundStyle(Tokens.color(.label2))
                    .rotationEffect(.degrees(open ? 90 : 0))
                    .accessibilityHidden(true)
                Text(verbatim: group.projectName)
                    .font(TypeStyle.headline)
                    .lineLimit(1)
                if pinned {
                    Image(systemName: "pin.fill")
                        .imageScale(.small)
                        .foregroundStyle(Tokens.color(.label2))
                        .accessibilityLabel("Pinned")
                }
                Spacer(minLength: Tokens.space[1])
                Text(verbatim: group.hostName + " · " + String(group.rows.count))
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                    .fixedSize()
            }
            HStack(spacing: Tokens.space[1]) {
                CountBar(parts: counts.bar)
                HStack(spacing: 0) {
                    if !counts.alert.isEmpty {
                        Text(verbatim: counts.alert)
                            .fontWeight(.semibold)
                            .foregroundStyle(counts.tone.color)
                        if !counts.rest.isEmpty {
                            Text(verbatim: " · " + counts.rest).foregroundStyle(Tokens.color(.label2))
                        }
                    } else {
                        Text(verbatim: counts.rest).foregroundStyle(Tokens.color(.label2))
                    }
                }
                .font(TypeStyle.footnote)
                .lineLimit(1)
            }
            .padding(.leading, 20)
        }
        .foregroundStyle(Tokens.color(.label))
        .padding(.vertical, Tokens.space[0])
        .contentShape(Rectangle())
    }
}

/// An agent in the attention card: its id, its project and its health with how long.
struct AttentionAgentRow: View {
    let row: AgentRow
    let projectName: String

    var body: some View {
        HStack(spacing: Tokens.space[2]) {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: row.agentId).font(TypeStyle.headline)
                Text(verbatim: projectName + " · " + row.role)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
            }
            Spacer(minLength: Tokens.space[1])
            StatePill(word: row.healthWord, symbol: row.healthSymbol, tone: AgentTone.tone(row.health)).fixedSize()
        }
        .accessibilityElement(children: .combine)
    }
}

/// The agents of one project, as the Agents tab groups them.
struct AgentGroup: Identifiable {
    let hostId: UUID
    let projectId: String
    let projectName: String
    let hostName: String
    let rows: [AgentRow]
    var id: String { hostId.uuidString + "/" + projectId }
}

extension AppModel {
    /// The agents of every ready project, by host name and then project name.
    func agentGroups() -> [AgentGroup] {
        var groups: [AgentGroup] = []
        for host in hosts {
            guard let projects = hostModels[host.id]?.projects.values else { continue }
            for project in projects where project.availability == .ready && !project.agents.isEmpty {
                groups.append(AgentGroup(
                    hostId: host.id, projectId: project.projectId,
                    projectName: TextBudget.displayPrefix(project.displayName, maxBytes: 64).text,
                    hostName: host.displayName, rows: AgentRow.rows(project)))
            }
        }
        return groups.sorted { left, right in
            if left.hostName != right.hostName {
                return left.hostName.localizedStandardCompare(right.hostName) == .orderedAscending
            }
            return left.projectName.localizedStandardCompare(right.projectName) == .orderedAscending
        }
    }
}
