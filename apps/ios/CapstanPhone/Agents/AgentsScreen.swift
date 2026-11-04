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
        let groups = model.agentGroups().sorted(by: AgentShow.byName)
        let all = groups.flatMap { $0.rows }
        let needle = query.trimmingCharacters(in: .whitespaces)
        let filtering = !needle.isEmpty || show != .all
        let pinned = ProjectPins.list(pinnedRaw)
        let shown: [ShownGroup] = groups.compactMap { group in
            let rows = AgentShow.matching(group, show: show, needle: needle)
            return rows.isEmpty ? nil : ShownGroup(group: group, rows: rows)
        }
        let pinnedGroups = pinned.compactMap { id in shown.first { $0.group.id == id } }
        let otherGroups = shown.filter { !pinned.contains($0.group.id) }
        let attention = AgentShow.attention(groups)

        ScrollViewReader { proxy in
            List {
                Section {
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        Text(verbatim: AgentShow.summary(all.count, projects: groups.count))
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
                    entries: groups.map(\.jumpEntry),
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
}

/// A project and the agents of it the screen shows.
private struct ShownGroup {
    let group: AgentGroup
    let rows: [AgentRow]
}

extension HealthCounts {
    /// The bar's segments with their colours.
    var bar: [(count: Int, color: Color)] {
        let colors = [
            Tokens.color(.accent), Tokens.color(.red), Tokens.color(.green), Tokens.color(.blue),
            Tokens.color(.label2).opacity(0.35),
        ]
        return Array(zip(barCounts, colors)).map { (count: $0.0, color: $0.1) }
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

extension AppModel {
    /// The agents of every ready project, by host name and then project name.
    func agentGroups() -> [AgentGroup] {
        AgentGroup.make(hosts: hosts, hostModels: hostModels)
    }
}
