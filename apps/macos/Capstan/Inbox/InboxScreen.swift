import CapstanAppCore
import CapstanKit
import SwiftUI

/// Waiting for you: the items of every host and project, grouped by kind.
struct InboxScreen: View {
    let model: AppModel

    var body: some View {
        let groups = model.inbox()
        VStack(alignment: .leading, spacing: 0) {
            header(groups)
            ForEach(model.hosts) { config in
                if !isLive(config.id) {
                    ConnectionBanners(model: model, hostId: config.id)
                        .padding(.horizontal, Tokens.space[3])
                        .padding(.bottom, Tokens.space[1])
                }
            }
            if groups.isEmpty {
                placeholder
            } else {
                list(groups)
            }
        }
    }

    private func header(_ groups: [InboxGroup]) -> some View {
        HStack {
            Text("Waiting for you").font(.system(size: 20, weight: .semibold))
            Text(verbatim: String(groups.reduce(0) { $0 + $1.count }))
                .font(.system(size: 13).monospacedDigit())
                .foregroundStyle(Tokens.color(.label2))
            Spacer()
            Picker("Show", selection: kindBinding) {
                Text("All").tag(WaitingKind?.none)
                Text("Commands").tag(WaitingKind?.some(.op))
                Text("Prompts").tag(WaitingKind?.some(.prompt))
                Text("Designs").tag(WaitingKind?.some(.design))
                Text("Messages").tag(WaitingKind?.some(.message))
                Text("Merges").tag(WaitingKind?.some(.integration))
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(maxWidth: 420)
        }
        .padding(Tokens.space[3])
    }

    private var kindBinding: Binding<WaitingKind?> {
        Binding(get: { model.inboxKind }, set: { model.inboxKind = $0 })
    }

    private var selectionBinding: Binding<InboxRowID?> {
        Binding(get: { model.selectedRow }, set: { model.selectedRow = $0 })
    }

    private func isLive(_ hostId: UUID) -> Bool {
        if case .live? = model.connection[hostId] { return true }
        return false
    }

    /// True until the hosts that are connecting have sent their first snapshot.
    private var loading: Bool {
        model.hosts.contains { config in
            switch model.connection[config.id] {
            case nil, .connecting?: return true
            default:
                return model.hostModels[config.id]?.projects.values.contains { !$0.snapshotComplete && $0.availability == .ready } ?? false
            }
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        if loading {
            VStack(spacing: Tokens.space[1]) {
                ForEach(0..<4, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: Tokens.radius.row)
                        .fill(Tokens.color(.fill2))
                        .frame(height: 44)
                }
            }
            .padding(Tokens.space[3])
            .accessibilityLabel("Loading")
            Spacer()
        } else {
            VStack(spacing: Tokens.space[1]) {
                Image(systemName: "checkmark.circle").font(.system(size: 28)).foregroundStyle(Tokens.color(.label2))
                Text("Nothing is waiting for you").font(.system(size: 15, weight: .semibold))
                Text("New items appear here as soon as a project asks.")
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func list(_ groups: [InboxGroup]) -> some View {
        List(selection: selectionBinding) {
            ForEach(groups) { group in
                Section {
                    ForEach(group.rows) { row in
                        InboxRowView(row: row, forwarded: model.forwardShown(row.id), selected: model.selectedRow == row.id)
                            .tag(row.id)
                    }
                } header: {
                    KindGroupHeader(kind: group.kind, count: group.count)
                }
            }
        }
        .listStyle(.inset)
        .onKeyPress(.return) {
            guard model.selectedRow == nil, let first = groups.first?.rows.first else { return .handled }
            model.selectedRow = first.id
            return .handled
        }
    }
}

#if DEBUG
#Preview("InboxScreen") {
    InboxScreen(model: InboxPreviewFixtures.model())
        .frame(width: 560, height: 640)
}

#Preview("InboxScreen, stale and no decide") {
    InboxScreen(model: InboxPreviewFixtures.model(decide: false, stale: true))
        .frame(width: 560, height: 640)
}

#Preview("InboxScreen, empty") {
    InboxScreen(model: InboxPreviewFixtures.emptyModel())
        .frame(width: 560, height: 480)
}
#endif
