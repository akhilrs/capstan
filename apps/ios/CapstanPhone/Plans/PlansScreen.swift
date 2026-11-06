import CapstanAppCore
import CapstanKit
import SwiftUI

/// The Plans tab, redesigned for many plans: project chips (All, then pinned projects, then A to Z) narrow the list
/// to one project; a "Needs you" card lists the plans with integration items waiting; Active, In review and Done
/// split the plans by state. For all projects, plans are grouped by project, each group collapsed to one row with
/// its plan count, a bar and how many packages are integrated. For one project, Active is grouped by state (waits
/// for you, in progress, drafts). Search matches plan titles and ids.
struct PlansScreen: View {
    @Environment(AppModel.self) private var model
    /// The chosen project (`hostId/projectId`), or nil for all.
    @State private var scope: String?
    @State private var show: PlanShow = .active
    @State private var query = ""
    /// Groups whose open state differs from their default, keyed by scope, tab and group.
    @State private var flipped: Set<String> = []
    /// Groups whose every plan is shown.
    @State private var showingAll: Set<String> = []
    @State private var jumping = false
    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""
    @AppStorage(ProjectPins.recentKey) private var recentRaw = ""

    /// How many plans an open group shows before "Show N more".
    private static let firstPlans = 10

    var body: some View {
        let groups = model.planGroups()
        let pinned = ProjectPins.list(pinnedRaw)
        let ordered = PlanShow.ordered(groups, pinned: pinned)
        let scoped = ordered.filter { scope == nil || $0.id == scope }
        let items = scoped.flatMap { group in
            group.rows.reversed().map { CapstanAppCore.PlanItem(group: group, row: $0, waiting: group.waiting($0)) }
        }
        let needle = query.trimmingCharacters(in: .whitespaces)
        let searched = needle.isEmpty ? items : items.filter {
            $0.row.title.localizedCaseInsensitiveContains(needle) || $0.row.planId.localizedCaseInsensitiveContains(needle)
        }
        let needs = items.filter { $0.waiting > 0 && $0.row.hasUnintegratedWork }
        let sections = PlanShow.sections(
            searched.filter { PlanShow.of($0.row) == show }, groups: scoped, scope: scope, show: show)

        List {
            Section {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    Text(verbatim: PlanShow.summary(items.count, projects: scoped.count, scope: scoped.first?.projectName, all: scope == nil))
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                    if groups.count > 1 {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: Tokens.space[1]) {
                                chip(id: nil, label: "All", count: groups.reduce(0) { $0 + $1.rows.count })
                                ForEach(ordered) { group in
                                    chip(id: group.id, label: group.projectName, count: group.rows.count)
                                }
                            }
                        }
                        .scrollClipDisabled()
                    }
                }
                .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }

            if needle.isEmpty && !needs.isEmpty {
                Section {
                    ForEach(needs) { item in
                        NavigationLink(value: item.route) {
                            NeedsYouPlanRow(item: item, showProject: scope == nil)
                        }
                    }
                } header: {
                    Text(verbatim: "Needs you · " + String(needs.count))
                        .foregroundStyle(Tokens.color(.accent))
                }
            }

            if !groups.isEmpty {
                Section {
                    Picker(selection: $show) {
                        ForEach(PlanShow.allCases, id: \.self) { option in
                            Text(verbatim: option.title + " " + String(searched.filter { PlanShow.of($0.row) == option }.count))
                                .tag(option)
                        }
                    } label: {
                        Text("Show")
                    }
                    .pickerStyle(.segmented)
                    .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                }
            }

            if groups.isEmpty {
                Section {
                    ContentUnavailableView(
                        "No plans", systemImage: "list.bullet.rectangle",
                        description: Text("The plans of your ready projects appear here."))
                }
                .listRowBackground(Color.clear)
            } else if sections.isEmpty {
                Section {
                    if needle.isEmpty {
                        ContentUnavailableView(
                            "Nothing here", systemImage: "list.bullet.rectangle",
                            description: Text("No plan is in this state."))
                    } else {
                        ContentUnavailableView.search(text: needle)
                    }
                }
                .listRowBackground(Color.clear)
            }

            ForEach(Array(sections.enumerated()), id: \.element.id) { index, section in
                groupSection(section, searching: !needle.isEmpty, single: sections.count == 1,
                             header: index == 0 ? PlanShow.groupTitle(scope: scope, show: show) : nil)
            }

            if !sections.isEmpty {
                Section {
                } footer: {
                    Text("The bar: green integrated, blue reported, grey assigned, empty not started.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .listSectionSpacing(Tokens.space[1])
        .scrollContentBackground(.hidden)
        .background(Tokens.color(.ground))
        .navigationTitle("Plans")
        .navigationBarTitleDisplayMode(.large)
        .searchable(text: $query, prompt: Text("Plan titles and ids"))
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    jumping = true
                } label: {
                    Label("Jump to project", systemImage: "list.bullet")
                }
                .disabled(groups.count < 2)
            }
        }
        .onChange(of: show) { _, _ in Haptics.play(.selection) }
        .onChange(of: groups.map(\.id)) { _, ids in
            if let current = scope, !ids.contains(current) { scope = nil }
        }
        .sheet(isPresented: $jumping) {
            ProjectJumpSheet(
                entries: groups.map(\.jumpEntry),
                pinned: pinned,
                recent: ProjectPins.list(recentRaw),
                togglePin: { pinnedRaw = ProjectPins.toggled($0, in: pinnedRaw) },
                pick: { id in
                    scope = id
                    recentRaw = ProjectPins.touched(id, in: recentRaw)
                    jumping = false
                })
        }
    }

    private func chip(id: String?, label: String, count: Int) -> some View {
        let on = scope == id
        return Button {
            Haptics.play(.selection)
            scope = id
            if let id { recentRaw = ProjectPins.touched(id, in: recentRaw) }
        } label: {
            HStack(spacing: Tokens.space[0]) {
                Text(verbatim: label).lineLimit(1)
                Text(verbatim: String(count)).monospacedDigit().opacity(0.8)
            }
            .font(TypeStyle.footnote.weight(.semibold))
            .foregroundStyle(on ? Tokens.color(.onAccent) : Tokens.color(.label2))
            .padding(.horizontal, Tokens.space[2])
            .frame(minHeight: Tokens.chipHeight)
            .background(on ? Tokens.color(.accent) : Color.clear, in: RoundedRectangle(cornerRadius: Tokens.radius.control))
            .overlay(RoundedRectangle(cornerRadius: Tokens.radius.control)
                .strokeBorder(on ? Tokens.color(.accent) : Tokens.color(.sep), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// One group: its summary row (a button that opens or closes it), then its plans when open.
    @ViewBuilder
    private func groupSection(_ section: PlanSection, searching: Bool, single: Bool, header: String?) -> some View {
        let key = (scope ?? "all") + "|" + show.title + "|" + section.id
        let open = single || searching || (section.openByDefault != flipped.contains(key))
        let every = searching || showingAll.contains(key)
        let rows = every ? section.items : Array(section.items.prefix(PlansScreen.firstPlans))
        Section {
            Button {
                Haptics.play(.selection)
                withAnimation(.easeInOut(duration: Motion.state)) {
                    if flipped.contains(key) { flipped.remove(key) } else { flipped.insert(key) }
                }
            } label: {
                PlanGroupRow(section: section, open: open)
            }
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(open ? Text("Open") : Text("Closed"))
            if open {
                ForEach(rows) { item in
                    NavigationLink(value: item.route) {
                        PlanCompactRow(item: item)
                    }
                }
                if rows.count < section.items.count {
                    Button {
                        showingAll.insert(key)
                    } label: {
                        Text(verbatim: "Show " + String(section.items.count - rows.count) + " more")
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
}

extension CapstanAppCore.PlanItem {
    var route: PhoneRoute {
        PhoneRoute.plan(PlanRef(hostId: group.hostId, projectId: group.projectId, planId: row.planId))
    }
}

/// The summary row of a group: a chevron, its title, host and plan count, then a bar and how many packages are
/// integrated.
struct PlanGroupRow: View {
    let section: PlanSection
    let open: Bool

    var body: some View {
        let rows = section.items.map(\.row)
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: "chevron.right")
                    .font(TypeStyle.footnote.weight(.semibold))
                    .foregroundStyle(Tokens.color(.label2))
                    .rotationEffect(.degrees(open ? 90 : 0))
                    .accessibilityHidden(true)
                Text(verbatim: section.title)
                    .font(TypeStyle.headline)
                    .foregroundStyle(section.tone?.color ?? Tokens.color(.label))
                    .lineLimit(1)
                Spacer(minLength: Tokens.space[1])
                Text(verbatim: (section.subtitle.map { $0 + " · " } ?? "") + String(section.items.count))
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
                    .fixedSize()
            }
            if section.total > 0 {
                HStack(spacing: Tokens.space[1]) {
                    CountBar(parts: [
                        (Int(rows.reduce(0) { $0 + $1.integrated }), Tokens.color(.green)),
                        (Int(rows.reduce(0) { $0 + $1.reportedOnly }), Tokens.color(.blue)),
                        (Int(rows.reduce(0) { $0 + $1.assignedOnly }), Tokens.color(.label2)),
                        (Int(rows.reduce(0) { $0 + $1.notStarted }), Tokens.color(.fill2)),
                    ], width: 64)
                    Text(verbatim: String(section.integrated) + " of " + String(section.total) + " packages integrated")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                        .lineLimit(1)
                }
                .padding(.leading, 20)
            }
        }
        .foregroundStyle(Tokens.color(.label))
        .padding(.vertical, Tokens.space[0])
        .contentShape(Rectangle())
    }
}

/// A plan in a group: its id and title with a pill when it means something, the bar, then the meta line.
struct PlanCompactRow: View {
    let item: CapstanAppCore.PlanItem

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Text(verbatim: item.row.planId)
                    .font(TypeStyle.monoSmall)
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize()
                Text(verbatim: item.row.title)
                    .font(TypeStyle.headline)
                    .lineLimit(1)
                Spacer(minLength: 0)
                if let pill = item.row.pill(waiting: item.waiting) {
                    StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
                }
            }
            PlanProgressBar(progress: item.row.progress)
                .frame(height: 4)
                .accessibilityHidden(true)
            Text(verbatim: item.row.metaLine)
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(1)
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }
}

/// A plan in the "Needs you" card: its title, id and project, and how many items wait.
struct NeedsYouPlanRow: View {
    let item: CapstanAppCore.PlanItem
    let showProject: Bool

    var body: some View {
        HStack(spacing: Tokens.space[2]) {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: item.row.title).font(TypeStyle.headline).lineLimit(1)
                Text(verbatim: showProject ? item.row.planId + " · " + item.group.projectName : item.row.planId)
                    .font(TypeStyle.monoSmall)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
            }
            Spacer(minLength: Tokens.space[1])
            if let pill = item.row.pill(waiting: item.waiting) {
                StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
            }
        }
        .accessibilityElement(children: .combine)
    }
}

extension AppModel {
    /// The plans of every ready project, by host name and then project name.
    func planGroups() -> [PlanGroup] {
        PlanGroup.make(hosts: hosts, hostModels: hostModels)
    }
}

/// A plan in the list: its title and id, its bar, the meta line, and a pill when it means something.
struct PlanCard: View {
    let row: PlanRow
    let waiting: Int

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: row.title).font(TypeStyle.headline).lineLimit(2)
                Spacer(minLength: Tokens.space[1])
                Text(verbatim: row.planId).font(TypeStyle.monoSmall).foregroundStyle(Tokens.color(.label2))
            }
            PlanProgressBar(progress: row.progress)
                .accessibilityHidden(true)
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: row.metaLine)
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(2)
                Spacer(minLength: 0)
                if let pill = row.pill(waiting: waiting) {
                    StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
                }
            }
        }
        .padding(.vertical, Tokens.space[0])
        .accessibilityElement(children: .combine)
    }
}

/// Four stages in one bar: integrated, reported, assigned, not started. The words beside it say the same counts.
struct PlanProgressBar: View {
    let progress: PlanProgress

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: 0) {
                segment(progress.integrated, Tokens.color(.green), geo.size.width)
                segment(progress.reported, Tokens.color(.blue), geo.size.width)
                segment(progress.assigned, Tokens.color(.label2), geo.size.width)
                Spacer(minLength: 0)
            }
        }
        .frame(height: 6)
        .background(Tokens.color(.fill2), in: Capsule())
        .clipShape(Capsule())
    }

    private func segment(_ share: Double, _ color: Color, _ width: CGFloat) -> some View {
        Rectangle().fill(color).frame(width: max(0, width * share))
    }
}
