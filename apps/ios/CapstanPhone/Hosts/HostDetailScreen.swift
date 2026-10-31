import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// A host's details (boards 12 and 13): its status line, why it is not live with the fix, the facts (address,
/// Capstan version and protocol, the trusted host key, the connection), its settings, a connection test, the exit
/// codes when it could not connect, and Remove host…, which confirms first with Cancel as the default.
struct HostDetailScreen: View {
    @Environment(AppModel.self) private var model
    let hostId: UUID

    @State private var confirmRemove = false
    @State private var testing = false

    var body: some View {
        if let host = model.hosts.first(where: { $0.id == hostId }) {
            content(host)
        } else {
            ContentUnavailableView("This host was removed", systemImage: "server.rack")
                .background(Tokens.color(.ground))
        }
    }

    private func content(_ host: HostConfig) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: "server.rack").foregroundStyle(Tokens.color(.accent)).accessibilityHidden(true)
                        Text("Host").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                    }
                    Text(verbatim: host.displayName).font(TypeStyle.title2).accessibilityAddTraits(.isHeader)
                    statusLine(host)
                }
                HostBanners(host: host)
                DetailFacts(facts: facts(host))
                if case .stopped(.unsupportedVersion(let version))? = model.connection[hostId] {
                    DetailFacts(facts: [
                        DetailFact(label: "Host sends", value: "version " + String(version)),
                        DetailFact(label: "App reads", value: "version 1"),
                    ])
                    Text("Update the app from the App Store. If no update exists yet, the host keeps running; answer there in the terminal.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                }
                settingsCell(host)
                if showsExitCodes {
                    ExitCodesCell()
                }
                Button(role: .destructive) { confirmRemove = true } label: {
                    Text("Remove host…")
                        .font(TypeStyle.body)
                        .foregroundStyle(Tokens.color(.red))
                        .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
                        .padding(.horizontal, Tokens.space[3])
                        .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
                }
                .buttonStyle(.plain)
                Text("Removing forgets the host key and stops following its projects. The key on the host stays until you delete it there.")
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button { model.navigation.sheet = .editHost(hostId) } label: { Label("Edit host", systemImage: "pencil") }
                    Button { model.restart(hostId: hostId) } label: { Label("Reconnect", systemImage: "arrow.clockwise") }
                    Divider()
                    Button(role: .destructive) { confirmRemove = true } label: {
                        Label("Remove host…", systemImage: "trash")
                    }
                } label: {
                    Label("More actions", systemImage: "ellipsis")
                }
            }
        }
        .sheet(isPresented: $confirmRemove) {
            ConfirmSheet(
                title: "Remove " + host.displayName + "?", detail: nil, noteLimit: nil, destructiveLabel: "Remove host",
                footnote: "Removing forgets the host key and stops following its projects. The key on the host stays until you delete it there.",
                onConfirm: { _ in model.removeHost(hostId) })
        }
        .sheet(isPresented: $testing) {
            HostCheckSheet(config: host, destination: model.phoneHost(hostId)?.display ?? host.displayName)
        }
    }

    private func statusLine(_ host: HostConfig) -> some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let line = HostStatusText.line(
                model.connection[hostId], name: host.displayName, problem: model.problems[hostId], now: context.date)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                HStack(spacing: Tokens.space[1]) {
                    Circle().fill(HostStatusText.color(line.tone)).frame(width: 8, height: 8).accessibilityHidden(true)
                    Text(verbatim: line.title).font(TypeStyle.subheadline.weight(.semibold))
                }
                Text(verbatim: line.detail).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
            .accessibilityElement(children: .combine)
        }
    }

    private func facts(_ host: HostConfig) -> [DetailFact] {
        var rows: [DetailFact] = []
        if let phone = model.phoneHost(hostId) {
            rows.append(DetailFact(label: "Address", value: phone.user + "@" + phone.hostname + ":" + String(phone.port), mono: true))
        }
        if let hello = model.hostModels[hostId]?.hello, hello.version.verified {
            let version = TextBudget.displayPrefix(hello.version.displayPlain, maxBytes: 32).text
            rows.append(DetailFact(label: "Capstan", value: version + " · protocol v1"))
        }
        if let pin = model.trustedPin(hostId) {
            rows.append(DetailFact(label: "Host key", value: pin.fingerprint, mono: true))
        } else {
            rows.append(DetailFact(label: "Host key", value: "Not trusted yet"))
        }
        rows.append(DetailFact(label: "cstan", value: host.cstanPath, mono: true))
        return rows
    }

    private func settingsCell(_ host: HostConfig) -> some View {
        VStack(spacing: 0) {
            settingRow("Extra project folders", value: String(host.extraRoots.count)) {
                model.navigation.sheet = .editHost(hostId)
            }
            Divider().padding(.leading, Tokens.space[3])
            settingRow("Poll interval", value: String(host.pollMs) + " ms") {
                model.navigation.sheet = .editHost(hostId)
            }
            Divider().padding(.leading, Tokens.space[3])
            settingRow("Test connection", value: nil) { testing = true }
        }
        .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
    }

    private func settingRow(_ title: String, value: String?, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(verbatim: title).font(TypeStyle.body).foregroundStyle(Tokens.color(.label))
                Spacer()
                if let value {
                    Text(verbatim: value).font(TypeStyle.body.monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                    Image(systemName: "chevron.right")
                        .font(TypeStyle.footnote.weight(.semibold))
                        .foregroundStyle(Tokens.color(.label2))
                        .accessibilityHidden(true)
                }
            }
            .padding(.horizontal, Tokens.space[3])
            .frame(minHeight: 52)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// The exit codes help when the host could not be reached or the stream stopped.
    private var showsExitCodes: Bool {
        switch model.connection[hostId] {
        case .reconnecting?: return true
        case .stopped(let reason)?: return reason != .user
        default: return false
        }
    }
}

/// What the exit codes of the stream mean (board 13).
struct ExitCodesCell: View {
    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text("Exit codes").font(TypeStyle.headline)
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: Tokens.space[3], verticalSpacing: Tokens.space[1]) {
                row("0", "closed on purpose")
                row("1", "an error line was sent first")
                row("2", "bad command line: update the app")
                row("255", "SSH itself failed")
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        }
    }

    private func row(_ code: String, _ meaning: String) -> some View {
        GridRow {
            Text(verbatim: code).font(TypeStyle.monoSmall).foregroundStyle(Tokens.color(.label2))
            Text(verbatim: meaning).font(TypeStyle.subheadline)
        }
    }
}

/// Test connection: the same checks as adding a host, run again on demand; nothing of them is kept.
struct HostCheckSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let config: HostConfig
    let destination: String

    @State private var checks: [HostCheck] = []
    @State private var running = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 0) {
                    row("Connected over SSH", .connect)
                    Divider()
                    row("Capstan answers", .capstan)
                    Divider()
                    row("Projects found", .projects)
                    Divider()
                    row("First snapshot", .firstSnapshot)
                }
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
                .padding(Tokens.margin)
            }
            .background(Tokens.color(.ground))
            .navigationTitle("Test connection")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Run again") { Task { await run() } }.disabled(running)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task { await run() }
    }

    private func row(_ title: String, _ step: HostCheckStep) -> some View {
        let found = checks.last { $0.step == step }
        let state: CheckRow.State
        switch found?.status {
        case .passed?: state = .passed
        case .warning?: state = .warning
        case .failed?: state = .failed
        case nil: state = running ? .running : .waiting
        }
        return CheckRow(title: title, state: state, detail: found.map { CheckStepText.make($0, destination: destination).line })
    }

    private func run() async {
        guard !running else { return }
        running = true
        checks = []
        for await step in model.check(config) { checks.append(step) }
        running = false
        if checks.contains(where: { $0.status == .failed }) { Haptics.play(.error) } else { Haptics.play(.success) }
    }
}

#Preview("Host detail") {
    NavigationStack { HostDetailScreen(hostId: PreviewData.alphaId) }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Host detail, reconnecting") {
    NavigationStack { HostDetailScreen(hostId: PreviewData.betaId) }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Host detail, problems") {
    NavigationStack { HostDetailScreen(hostId: PreviewData.betaId) }
        .environment(AppModel(preview: PreviewData.problems))
}
