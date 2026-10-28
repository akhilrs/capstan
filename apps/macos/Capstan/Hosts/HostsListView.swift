import CapstanAppCore
import CapstanKit
import SwiftUI

/// The configured hosts with their connection state, and the buttons to add, edit and remove one.
struct HostsListView: View {
    let model: AppModel
    @State private var removing: HostConfig?

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            if model.hosts.isEmpty {
                Text("No hosts yet. Add one to see its projects.")
                    .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            }
            ForEach(model.hosts) { host in
                row(host)
            }
            Button {
                if model.presented == nil { model.presented = .addHost(editing: nil) }
            } label: {
                Label("Add host…", systemImage: "plus")
            }
        }
        .confirmationDialog(
            "Remove this host?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            presenting: removing
        ) { host in
            Button("Remove", role: .destructive) { model.removeHost(host.id) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("The app forgets the host and what it saved of it. The host itself is not changed.")
        }
    }

    private func row(_ host: HostConfig) -> some View {
        HStack(spacing: Tokens.space[2]) {
            VStack(alignment: .leading, spacing: 0) {
                Text(verbatim: HostLabel.hostName(host)).font(.system(size: 13, weight: .medium)).lineLimit(1)
                Text(verbatim: HostLabel.plain(host.destination))
                    .font(.system(size: 12, design: .monospaced)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
            }
            Spacer(minLength: Tokens.space[1])
            if host.enabled {
                HostLabel.pill(model.connection[host.id])
            } else {
                StatePill(word: "Disabled", symbol: "pause.circle", tone: .grey)
            }
            Button("Edit…") {
                if model.presented == nil { model.presented = .addHost(editing: host) }
            }
            Button("Remove…", role: .destructive) { removing = host }
        }
        .padding(Tokens.space[1])
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .contentShape(Rectangle())
        .onTapGesture { model.hostsFocus = HostsFocus(hostId: host.id) }
        .accessibilityElement(children: .contain)
    }
}

#Preview("Hosts list") {
    HostsListView(model: AppModel(preview: PreviewData.standard))
        .padding().frame(width: 560)
}

#Preview("Hosts list: empty") {
    HostsListView(model: AppModel(preview: PreviewSeed()))
        .padding().frame(width: 560)
}
