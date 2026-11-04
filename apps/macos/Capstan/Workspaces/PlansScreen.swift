import CapstanAppCore
import CapstanKit
import SwiftUI

/// The plans of every project, or of one: the plans that wait for you first; then Active, In review or Done. For all
/// projects the plans are grouped by project, each group folded to one row with its bar; for one project, Active is
/// grouped by state (waits for you, in progress, drafts). A group shows ten plans, then "Show N more". Rows are lazy.
struct PlansScreen: View {
    let model: AppModel
    /// The project in view, or nil for all.
    let scope: ProjectKey?
    let show: PlanShow
    let query: String
    let onShow: (PlanShow) -> Void
    let onClearSearch: () -> Void

    @AppStorage(ProjectPins.pinnedKey) private var pinnedRaw = ""
    /// Groups whose open state differs from their default, keyed by scope, tab and group.
    @State private var flipped: Set<String> = []
    /// Groups whose every plan is shown.
    @State private var showingAll: Set<String> = []

    /// How many plans an open group shows before "Show N more".
    private static let firstPlans = 10

    private enum Entry: Identifiable {
        case text(String, id: String)
        case title(String, attention: Bool, id: String)
        case needs(ListPlan)
        case header(PlanSection, key: String, open: Bool, items: [ListPlan])
        case plan(ListPlan)
        case more(key: String, hidden: Int)
        case legend

        var id: String {
            switch self {
            case .text(_, let id): return "x/" + id
            case .title(_, _, let id): return "t/" + id
            case .needs(let item): return "n/" + item.id
            case .header(_, let key, _, _): return "h/" + key
            case .plan(let item): return "p/" + item.id
            case .more(let key, _): return "m/" + key
            case .legend: return "legend"
            }
        }
    }

    var body: some View {
        let pinned = ProjectPins.list(pinnedRaw)
        let scoped = ListData.planGroups(model, scope: scope, pinned: pinned)
        let items = ListData.planItems(scoped)
        let needle = query.trimmingCharacters(in: .whitespaces)
        let searched = PlanShow.matching(items, needle: needle)
        let tab = searched.filter { PlanShow.of($0.row) == show }
        let scopeId = scope.map { $0.hostId.uuidString + "/" + $0.projectId }
        let sections = PlanShow.sections(tab, groups: scoped, scope: scopeId, show: show)

        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                if items.isEmpty {
                    ListEmptyState(
                        symbol: "doc.text", title: "No plans yet",
                        detail: scope == nil ? "The plans of your ready projects appear here." : "This project has no plans yet.")
                } else if tab.isEmpty {
                    emptyTab(needle: needle)
                } else {
                    ForEach(entries(
                        items: items, sections: sections, scoped: scoped, needle: needle)
                    ) { entry in
                        row(entry, pinned: pinned)
                    }
                }
                // The status capsule floats over the foot of the column; this space keeps the last row above it.
                Color.clear.frame(height: StatusCapsule.reservedHeight)
            }
            .padding(.horizontal, Tokens.space[1])
            .padding(.vertical, Tokens.space[1])
        }
    }

    private func emptyTab(needle: String) -> some View {
        if needle.isEmpty {
            return ListEmptyState(
                symbol: "line.3.horizontal.decrease.circle", title: "Nothing to show",
                detail: "No plan is " + show.title.lowercased() + ".", actionTitle: "Show active plans") { onShow(.active) }
        }
        return ListEmptyState(
            symbol: "magnifyingglass", title: "No results for \"" + needle + "\"",
            detail: "Search matches plan titles and ids.", actionTitle: "Clear search", action: onClearSearch)
    }

    private func entries(
        items: [ListPlan], sections: [PlanSection], scoped: [PlanGroup], needle: String
    ) -> [Entry] {
        var list: [Entry] = []
        list.append(.text(
            PlanShow.summary(
                items.count, projects: scoped.count, scope: scoped.first?.projectName, all: scope == nil), id: "summary"))
        let needs = PlanShow.needsYou(items)
        if needle.isEmpty && !needs.isEmpty {
            list.append(.title("Needs you · " + String(needs.count), attention: true, id: "needs"))
            list += needs.map { Entry.needs($0) }
        }
        list.append(.title(PlanShow.groupTitle(scope: scope.map { _ in "scope" }, show: show), attention: false, id: "kind"))
        let scopeKey = scope.map { $0.hostId.uuidString + "/" + $0.projectId } ?? "all"
        for section in sections {
            let key = scopeKey + "/" + show.title + "/" + section.id
            let base = needle.isEmpty ? section.openByDefault : true
            let open = base != flipped.contains(key)
            list.append(.header(section, key: key, open: open, items: section.items))
            guard open else { continue }
            let rows = showingAll.contains(key) || !needle.isEmpty
                ? section.items : Array(section.items.prefix(Self.firstPlans))
            list += rows.map { Entry.plan($0) }
            if rows.count < section.items.count { list.append(.more(key: key, hidden: section.items.count - rows.count)) }
        }
        list.append(.legend)
        return list
    }

    @ViewBuilder
    private func row(_ entry: Entry, pinned: [String]) -> some View {
        switch entry {
        case .text(let text, _):
            Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[0])
        case .title(let text, let attention, _):
            ListSectionTitle(text: text, attention: attention)
        case .needs(let item):
            needsRow(item)
        case .header(let section, let key, let open, let items):
            header(section, key: key, open: open, items: items, pinned: pinned)
        case .plan(let item):
            planRow(item.row, key: ProjectKey(hostId: item.group.hostId, projectId: item.group.projectId))
        case .more(let key, let hidden):
            Button { showingAll.insert(key) } label: {
                Text(verbatim: "Show " + String(hidden) + " more").foregroundStyle(Tokens.color(.accent))
            }
            .buttonStyle(.plain)
            .padding(.leading, Tokens.space[5] + Tokens.space[2])
            .padding(.vertical, Tokens.space[0])
        case .legend:
            legend
        }
    }

    // MARK: Rows

    private func needsRow(_ item: ListPlan) -> some View {
        let ref = PlanRef(hostId: item.group.hostId, projectId: item.group.projectId, planId: item.row.planId)
        return Button {
            model.selectedPlan = ref
        } label: {
            HStack(spacing: Tokens.space[2]) {
                Text(verbatim: item.row.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(verbatim: item.row.planId + (scope == nil ? " · " + item.group.projectName : ""))
                    .font(.system(size: 12, design: .monospaced)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                if let pill = item.row.pill(waiting: item.waiting) {
                    StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
                }
            }
            .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[1])
            .listRow(selected: model.selectedPlan == ref)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func header(_ section: PlanSection, key: String, open: Bool, items: [ListPlan], pinned: [String]) -> some View {
        let isProject = scope == nil
        Button {
            if flipped.contains(key) { flipped.remove(key) } else { flipped.insert(key) }
        } label: {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack(spacing: Tokens.space[1]) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold)).frame(width: 12)
                        .foregroundStyle(Tokens.color(.label2))
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .accessibilityHidden(true)
                    Text(verbatim: section.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                        .foregroundStyle(section.tone?.color ?? Tokens.color(.label))
                    if isProject && pinned.contains(section.id) {
                        Image(systemName: "pin.fill").imageScale(.small).foregroundStyle(Tokens.color(.label2))
                            .accessibilityLabel("Pinned")
                    }
                    Spacer(minLength: Tokens.space[1])
                    Text(verbatim: (section.subtitle.map { $0 + " · " } ?? "") + planWord(section.items.count))
                        .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                }
                if isProject {
                    HStack(spacing: Tokens.space[1]) {
                        PlanGroupBar(items: items)
                        Text(verbatim: String(section.integrated) + " of " + String(section.total) + " packages integrated")
                            .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    }
                    .padding(.leading, 20)
                }
            }
            .padding(.horizontal, Tokens.space[2]).padding(.vertical, Tokens.space[1])
            .listRow(selected: false)
        }
        .buttonStyle(.plain)
        .accessibilityValue(open ? "Open" : "Closed")
        .contextMenu {
            if isProject {
                Button {
                    pinnedRaw = ProjectPins.toggled(section.id, in: pinnedRaw)
                } label: {
                    Text(verbatim: (pinned.contains(section.id) ? "Unpin " : "Pin ") + section.title)
                }
                if let first = items.first {
                    Button {
                        model.selection = .plans(hostId: first.group.hostId, projectId: first.group.projectId)
                    } label: {
                        Text(verbatim: "Show only " + section.title)
                    }
                }
            }
        }
    }

    private func planWord(_ count: Int) -> String { count == 1 ? "1 plan" : String(count) + " plans" }

    private var legend: some View {
        HStack(spacing: Tokens.space[2]) {
            swatch(Tokens.color(.green), "integrated")
            swatch(Tokens.color(.blue), "reported")
            swatch(Tokens.color(.label2), "assigned")
            swatch(Tokens.color(.fill2), "not started")
        }
        .font(.system(size: 12))
        .foregroundStyle(Tokens.color(.label2))
        .padding(Tokens.space[2])
    }

    private func swatch(_ color: Color, _ word: String) -> some View {
        HStack(spacing: Tokens.space[0]) {
            RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 10, height: 10)
            Text(verbatim: word)
        }
    }

    private func planRow(_ row: PlanRow, key: ProjectKey) -> some View {
        let ref = PlanRef(hostId: key.hostId, projectId: key.projectId, planId: row.planId)
        let selected = model.selectedPlan == ref
        // Only a waiting item that a package of this plan names counts for it; none is claimed otherwise.
        let waiting = model.project(key.hostId, key.projectId).map {
            IntegrationLink.waiting(for: PlanRow.plan(for: row, in: $0), in: $0).count
        } ?? 0
        return Button {
            model.selectedPlan = ref
        } label: {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack(spacing: Tokens.space[2]) {
                    Text(verbatim: "plan-\(row.sequence)")
                        .font(.system(size: 12, design: .monospaced)).foregroundStyle(Tokens.color(.label2))
                        .frame(width: 64, alignment: .leading)
                    Text(verbatim: row.title).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    Spacer(minLength: Tokens.space[1])
                    if let pill = row.pill(waiting: waiting) {
                        StatePill(word: pill.word, symbol: nil, tone: pill.tone).fixedSize()
                    }
                }
                PlanProgressBar(progress: row.progress).padding(.leading, 64 + Tokens.space[2])
                Text(verbatim: row.metaLine).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
                    .padding(.leading, 64 + Tokens.space[2])
            }
            .padding(.leading, scope == nil ? Tokens.space[5] + Tokens.space[2] : Tokens.space[2])
            .padding(.trailing, Tokens.space[2]).padding(.vertical, Tokens.space[1])
            .listRow(selected: selected)
        }
        .buttonStyle(.plain)
    }
}

/// A project's plans as one small bar: integrated, reported and assigned packages out of all of them.
struct PlanGroupBar: View {
    let items: [ListPlan]

    var body: some View {
        let total = max(items.reduce(0) { $0 + $1.row.total }, 1)
        let integrated = items.reduce(0) { $0 + $1.row.integrated }
        let reported = items.reduce(0) { $0 + $1.row.reportedOnly }
        let assigned = items.reduce(0) { $0 + $1.row.assignedOnly }
        GeometryReader { geo in
            HStack(spacing: 0) {
                Rectangle().fill(Tokens.color(.green)).frame(width: geo.size.width * CGFloat(integrated) / CGFloat(total))
                Rectangle().fill(Tokens.color(.blue)).frame(width: geo.size.width * CGFloat(reported) / CGFloat(total))
                Rectangle().fill(Tokens.color(.label2)).frame(width: geo.size.width * CGFloat(assigned) / CGFloat(total))
                Spacer(minLength: 0)
            }
        }
        .frame(width: 64, height: 6)
        .background(Tokens.color(.fill2), in: Capsule())
        .clipShape(Capsule())
        .accessibilityHidden(true)
    }
}

/// Four stages in one bar: integrated, reported, assigned, not started.
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
        Rectangle().fill(color).frame(width: width * share)
    }
}

/// The plan the Plans screen selected: its header, its bar, its packages, the way to its agents and its waiting item,
/// and Cancel plan… (Cancel draft… for a draft) when the host can decide and sent the plan's binding.
struct PlanInspector: View {
    let model: AppModel

    @State private var confirming = false
    @State private var sending = false
    @State private var outcome: DecideOutcome?
    @State private var cancelledPlan: String?

    var body: some View {
        if let ref = model.selectedPlan, let project = model.project(ref.hostId, ref.projectId),
            let row = PlanRow.rows(project).first(where: { $0.planId == ref.planId })
        {
            let stored = PlanRow.plan(for: row, in: project)
            let packages = stored.map { PackageRow.rows($0, in: project) } ?? []
            let waiting = IntegrationLink.waiting(for: stored, in: project).first
            let hostName = model.hosts.first { $0.id == ref.hostId }.map(HostLabel.hostName) ?? "the host"
            let projectName = HostLabel.projectName(project)
            VStack(alignment: .leading, spacing: Tokens.space[2]) {
                ScrollView {
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        HStack {
                            Text(verbatim: "Plan · " + projectName + " · " + row.stateWord)
                                .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                            Spacer()
                            if let stored, cancellable(stored, row: row, hostId: ref.hostId) {
                                Menu {
                                    Button {
                                        model.selection = .agents(hostId: ref.hostId, projectId: ref.projectId)
                                    } label: { Text("Show agents") }
                                    Divider()
                                    Button(role: .destructive) { confirming = true } label: {
                                        Text(verbatim: row.state == "draft" ? "Cancel draft…" : "Cancel plan…")
                                    }
                                    .keyboardShortcut(.delete, modifiers: .command)
                                    .disabled(sending)
                                } label: {
                                    Image(systemName: "ellipsis.circle")
                                }
                                .menuStyle(.borderlessButton)
                                .fixedSize()
                                .accessibilityLabel("More actions")
                            }
                        }
                        Text(verbatim: "plan-\(row.sequence) · " + row.title).font(.system(size: 20, weight: .semibold))
                        if sending {
                            StatePill(word: "Cancelling…", symbol: "hourglass", tone: .grey)
                        } else if let outcome {
                            let shown = Presentation.present(outcome, hostName: hostName)
                            StatePill(word: shown.title, symbol: "info.circle", tone: shown.tone)
                        }
                        PlanProgressBar(progress: row.progress)
                        Text(verbatim: row.summaryLine).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                        if !packages.isEmpty {
                            PackageRowsView(plan: row, packages: packages).padding(.top, Tokens.space[1])
                        }
                    }
                }
                HStack {
                    Spacer()
                    Button {
                        model.selection = .agents(hostId: ref.hostId, projectId: ref.projectId)
                    } label: { Text("Show agents") }
                    .buttonStyle(.capstanQuiet)
                    if row.hasUnintegratedWork, let waiting {
                        Button {
                            model.showMain(row: InboxRowID(hostId: ref.hostId, projectId: ref.projectId, key: waiting.key))
                        } label: { Text("Open the waiting item") }
                        .buttonStyle(.capstanPrimary)
                    } else if row.hasUnintegratedWork, IntegrationLink.waitingCount(project) > 0 {
                        // The host did not say which item is this plan's: open the project's integration list.
                        Button {
                            model.showProject(hostId: ref.hostId, projectId: ref.projectId, kind: .integration)
                        } label: { Text("Open the waiting item") }
                        .buttonStyle(.capstanPrimary)
                    }
                }
            }
            .padding(Tokens.space[3])
            .sheet(isPresented: $confirming) {
                if let stored {
                    CancelPlanSheet(
                        projectName: projectName, planId: row.planId, title: stored.title, stateLine: row.metaLine,
                        isDraft: row.state == "draft"
                    ) {
                        cancel(stored.key, ref: ref)
                    }
                }
            }
            .onChange(of: ref) { _, _ in
                outcome = nil
                sending = false
            }
        } else if let cancelledPlan {
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Image(systemName: "xmark.circle").font(.system(size: 28)).foregroundStyle(Tokens.color(.label2))
                    .accessibilityHidden(true)
                Text("Plan cancelled").font(.system(size: 15, weight: .semibold))
                Text(verbatim: cancelledPlan + ": its open packages were stopped and the PM was told.")
                    .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            Text("Select a plan to see its progress")
                .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// The plan can be cancelled from here: it is not finished, the host can decide and it sent the plan's binding.
    private func cancellable(_ stored: StoredPlan, row: PlanRow, hostId: UUID) -> Bool {
        (model.capabilities[hostId]?.decide ?? false) && stored.requestId != nil && stored.contentHash != nil
            && PlanFilter.filter(of: row) != .done
    }

    private func cancel(_ key: String, ref: PlanRef) {
        guard !sending else { return }
        sending = true
        Task {
            let result = await model.cancelPlan(key: key, hostId: ref.hostId, projectId: ref.projectId)
            outcome = result
            sending = false
            if case .applied = result { cancelledPlan = ref.planId }
        }
    }
}

#Preview("Plans: several progress points") {
    PlansScreen(
        model: AppModel(preview: PreviewData.standard), scope: nil, show: .active, query: "", onShow: { _ in },
        onClearSearch: {}
    )
    .frame(width: 520, height: 420)
}
