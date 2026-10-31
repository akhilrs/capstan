import CapstanAppCore
import CapstanKit
import SwiftUI
import UIKit

/// Boards 02 and 06, Waiting for you: every waiting item grouped by kind with counts, the live status line, the kind
/// chips and search, swipe left (Message PM, and Deny… on a proposal, never a full swipe), the long-press menu, and the
/// empty, first snapshot, reconnecting and nothing-reachable states. Approve is never a swipe or a menu item: it needs
/// the full text, on the item's screen.
struct WaitingScreen: View {
    @Environment(AppModel.self) private var model
    @State private var denying: InboxRow?

    var body: some View {
        @Bindable var model = model
        let groups = model.displayGroups()
        List {
            Section {
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    WaitingStatusLine()
                    KindChips(selected: $model.inboxKind)
                }
                .listRowInsets(EdgeInsets(top: 0, leading: Tokens.margin, bottom: 0, trailing: Tokens.margin))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
            stateBanner
            if groups.isEmpty {
                emptyState
            }
            ForEach(groups) { group in
                Section {
                    ForEach(group.rows) { row in
                        rowLink(row, quiet: group.quiet)
                    }
                } header: {
                    KindGroupHeader(title: group.header, quiet: group.quiet)
                }
            }
            if model.inboxKind != nil, !groups.isEmpty {
                Text(verbatim: "Showing " + KindTab(kind: model.inboxKind).title.lowercased()
                    + " only. Clear the filter to see all " + String(model.inboxTotal()) + " items.")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .listRowBackground(Color.clear)
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Tokens.color(.ground))
        .navigationTitle("Waiting for you")
        .navigationBarTitleDisplayMode(.large)
        .searchable(text: $model.inboxSearch, prompt: Text("Search projects and summaries"))
        .refreshable { refreshAll() }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    model.navigation.show(.messages, [])
                } label: {
                    Label("Message PM", systemImage: "square.and.pencil")
                }
            }
        }
        .sheet(item: $denying) { row in
            ConfirmSheet(
                title: "Deny this command?", detail: row.summary, noteLimit: ProtocolLimits.reasonCap,
                destructiveLabel: "Deny", detailLabel: "Command · " + row.projectName,
                noteLabel: "Note to the operator (optional)",
                footnote: "Deny is applied on the host at once and cannot be taken back from here.",
                onConfirm: { note in deny(row, note: note) })
        }
    }

    // MARK: Rows

    private func rowLink(_ row: InboxRow, quiet: Bool) -> some View {
        let answerable = !row.stale && model.connectionIsLive(row.id.hostId)
        return NavigationLink(value: PhoneRoute.item(row.id)) {
            WaitingRow(row: row, forwarded: model.forwardShown(row.id), widening: widening(row), quiet: quiet)
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if row.kind == .op, answerable {
                Button(role: .destructive) { denying = row } label: { Label("Deny…", systemImage: "xmark") }
            }
            Button { messagePM(row) } label: { Label("Message PM", systemImage: "bubble.left") }
                .tint(Tokens.color(.label2))
        }
        .contextMenu {
            Button { model.navigation.show(.waiting, [.item(row.id)]) } label: { Label("Open", systemImage: "arrow.up.forward.square") }
            if row.kind == .op, row.summary.verified {
                Button { copyCommand(row) } label: { Label("Copy command", systemImage: "doc.on.doc") }
            }
            Button { messagePM(row) } label: { Label("Message PM about this", systemImage: "bubble.left") }
            Button {
                model.navigation.show(.waiting, [.project(hostId: row.id.hostId, projectId: row.id.projectId)])
            } label: {
                Label { Text(verbatim: "Open " + row.projectName) } icon: { Image(systemName: "folder") }
            }
            if row.kind == .op, answerable {
                Divider()
                Button(role: .destructive) { denying = row } label: { Label("Deny…", systemImage: "xmark") }
            }
        } preview: {
            WaitingPreview(row: row)
        }
        .listRowBackground(Tokens.color(.cell))
    }

    private func widening(_ row: InboxRow) -> Int {
        guard row.kind == .prompt, let options = model.detail(row.id)?.promptBody?.options else { return 0 }
        return options.filter(\.widensPermissions).count
    }

    // MARK: States (board 06)

    @ViewBuilder private var stateBanner: some View {
        let states = model.hosts.map { model.connection[$0.id] }
        if !model.hosts.isEmpty, states.allSatisfy({ $0 == nil || WaitingScreen.isDown($0) }), model.hostModels.values.allSatisfy({ $0.projects.isEmpty }) {
            Banner(tone: .red, text: "No host can be reached", actionTitle: "Open Hosts") {
                model.navigation.show(.hosts, [])
            }
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets())
        } else if let down = model.hosts.first(where: { WaitingScreen.isReconnecting(model.connection[$0.id]) && model.hostModels[$0.id] != nil }) {
            Banner(tone: .yellow, text: "Reconnecting to " + down.displayName + ". Its items are as of the last read and cannot be answered until it is back.")
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
        }
    }

    @ViewBuilder private var emptyState: some View {
        if model.hosts.contains(where: { model.connection[$0.id] == .connecting }) && model.hostModels.values.allSatisfy({ $0.projects.isEmpty }) {
            Section {
                ForEach(0..<4, id: \.self) { _ in SkeletonRow() }
            } header: {
                Text("Reading the first snapshot…")
            }
        } else if model.inboxKind != nil || !model.inboxSearch.isEmpty {
            ContentUnavailableView("Nothing matches", systemImage: "line.3.horizontal.decrease.circle", description: Text("Clear the filter or the search to see every item."))
                .listRowBackground(Color.clear)
        } else {
            ContentUnavailableView("Nothing waits", systemImage: "tray", description: Text("What your agents wait for appears here."))
                .listRowBackground(Color.clear)
        }
    }

    static func isDown(_ state: ConnectionState?) -> Bool {
        switch state {
        case .stopped?, .reconnecting?: return true
        default: return false
        }
    }

    static func isReconnecting(_ state: ConnectionState?) -> Bool {
        if case .reconnecting? = state { return true }
        return false
    }

    // MARK: Actions

    private func refreshAll() {
        for host in model.hosts {
            model.refresh(hostId: host.id, projectIds: model.readyProjects(hostId: host.id))
        }
    }

    private func messagePM(_ row: InboxRow) {
        model.navigation.show(.messages, [.conversation(hostId: row.id.hostId, projectId: row.id.projectId)])
    }

    private func copyCommand(_ row: InboxRow) {
        UIPasteboard.general.string = row.summary.displayPlain
        Haptics.play(.success)
    }

    private func deny(_ row: InboxRow, note: String?) {
        Task {
            let outcome = await model.decide(.opDeny(note: note), on: row.id)
            switch outcome {
            case .applied, .forwarded: Haptics.play(.success)
            case .notSent: Haptics.play(.error)
            default: Haptics.play(.warning)
            }
        }
    }
}

extension AppModel {
    /// The host's stream is live: its items can be answered.
    func connectionIsLive(_ hostId: UUID) -> Bool {
        if case .live? = connection[hostId] { return problems[hostId] == nil }
        return false
    }
}

/// The long-press preview: the kind, project and age, and the summary as untrusted text.
private struct WaitingPreview: View {
    let row: InboxRow

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Label {
                Text(verbatim: WaitingPreview.kindName(row) + " · " + row.projectName + " · "
                    + AgeText.short(since: row.since, now: Date()))
            } icon: {
                Image(systemName: row.kind.glyph)
            }
            .font(TypeStyle.subheadline)
            .foregroundStyle(Tokens.color(.label2))
            UntrustedTextBlock(text: row.summary, label: row.kind == .op ? "Command" : "Summary")
        }
        .padding(Tokens.margin)
        .frame(width: 340)
        .background(Tokens.color(.cell))
    }

    /// The kind as the preview names it; an integration that did not merge is a merge conflict.
    static func kindName(_ row: InboxRow) -> String {
        if row.kind == .integration, RowText.integrationState(row) != .merged { return "Merge conflict" }
        return RowText.kindName(row.kind)
    }
}

/// The live status line under the title: one host by name with its heartbeat, or a count of hosts.
struct WaitingStatusLine: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: Tokens.space[1]) {
                Circle().fill(color).frame(width: 8, height: 8).accessibilityHidden(true)
                Text(verbatim: line(now: context.date))
                    .font(TypeStyle.subheadline)
                    .foregroundStyle(Tokens.color(.label2))
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var state: ConnectionState? {
        aggregateConnection(model.hosts.compactMap { model.connection[$0.id] })
    }

    private var color: Color {
        switch state {
        case .live?: return Tokens.color(.green)
        case .stopped?: return Tokens.color(.red)
        case nil: return Tokens.color(.label2)
        default: return Tokens.color(.yellow)
        }
    }

    private func line(now: Date) -> String {
        if model.hosts.isEmpty { return "No host yet" }
        let name = model.hosts.count == 1 ? model.hosts[0].displayName : String(model.hosts.count) + " hosts"
        switch state {
        case .live(_, let lastLineAt)?:
            let seconds = max(0, Int(now.timeIntervalSince(lastLineAt)))
            return name + " live · heartbeat " + String(seconds) + " s ago"
        case .connecting?: return name + " · connecting"
        case .noHeartbeat?: return name + " · no heartbeat"
        case .reconnecting?: return name + " · reconnecting"
        case .stopped?: return name + " · stopped"
        case nil: return name + " · not connected"
        }
    }
}

/// The kind chips: All · Proposals · Prompts · Designs · Messages · Merges, each with its count. The selected chip is
/// the one accent.
struct KindChips: View {
    @Environment(AppModel.self) private var model
    @Binding var selected: WaitingKind?

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: Tokens.space[1]) {
                ForEach(KindTab.visible, id: \.self) { tab in
                    let on = KindTab(kind: selected) == tab
                    Button {
                        selected = tab.kind
                        Haptics.play(.selection)
                    } label: {
                        Text(verbatim: tab.title + " " + String(count(tab)))
                            .font(TypeStyle.subheadline.weight(on ? .semibold : .regular))
                            .padding(.horizontal, Tokens.space[3])
                            .frame(minHeight: Tokens.chipHeight)
                            .foregroundStyle(on ? Tokens.color(.onAccent) : Tokens.color(.label))
                            .background(on ? Tokens.color(.accent) : Tokens.color(.fill2), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
        }
    }

    private func count(_ tab: KindTab) -> Int {
        guard let kind = tab.kind else { return model.inboxTotal() }
        return InboxScope(kind: kind, search: model.inboxSearch)
            .apply(CapstanKit.inbox(Array(model.hostModels.values)))
            .reduce(0) { $0 + $1.count }
    }
}

#Preview("Waiting, all") {
    NavigationStack { WaitingScreen() }.environment(AppModel(preview: PreviewData.standard))
}

#Preview("Waiting, dark") {
    NavigationStack { WaitingScreen() }.environment(AppModel(preview: PreviewData.standard)).preferredColorScheme(.dark)
}

#Preview("Waiting, filtered to merges") {
    let model = AppModel(preview: PreviewData.standard)
    model.inboxKind = .integration
    return NavigationStack { WaitingScreen() }.environment(model)
}

#Preview("Waiting, empty") {
    NavigationStack { WaitingScreen() }.environment(AppModel(preview: PreviewData.empty))
}
