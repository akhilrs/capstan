import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Board 01, Checks: the first connection of a candidate, step by step. A host key the phone has not seen opens the
/// trust sheet; after Trust the checks run again. The host is added only after the checks passed.
struct ChecksScreen: View {
    @Bindable var model: AppModel
    let candidate: HostCandidate
    let onAdded: () -> Void

    @State private var checks: [HostCheck] = []
    @State private var running = false
    @State private var added = false
    @Environment(\.dismiss) private var dismiss

    private var hostId: UUID { candidate.host.id }
    private var display: String { candidate.host.display }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text(verbatim: candidate.config.displayName).font(TypeStyle.largeTitle)
                VStack(spacing: 0) {
                    CheckRow(title: "Connected over SSH", state: state(of: .connect), detail: line(of: .connect))
                    Divider()
                    CheckRow(title: "Host key trusted", state: trustState, detail: nil)
                    Divider()
                    CheckRow(title: "Capstan answers", state: state(of: .capstan), detail: line(of: .capstan))
                    Divider()
                    CheckRow(title: "Projects found", state: state(of: .projects), detail: line(of: .projects))
                    Divider()
                    CheckRow(title: "First snapshot", state: state(of: .firstSnapshot), detail: line(of: .firstSnapshot))
                }
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
                if model.problems[hostId] == .authRefused, failed {
                    Banner(tone: .red, text: "The host refused this iPhone's key")
                    Text("Add this iPhone's public key to ~/.ssh/authorized_keys on the host, then check again.")
                        .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
                if case .unsupported? = model.problems[hostId], failed {
                    Banner(tone: .red, text: "The host offers no key type or algorithm this iPhone supports")
                }
                if check(.projects)?.status == .warning {
                    Banner(tone: .yellow, text: "Some projects are not ready")
                    Text("Create the operator key for each project that is not ready, on the host. The other projects work now.")
                        .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
            }
            .padding(.horizontal, Tokens.margin)
        }
        .background(Tokens.color(.ground))
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            ActionBar {
                if running {
                    Button(action: {}) { Text("Reading projects…") }
                        .buttonStyle(.capstanPrimary)
                        .disabled(true)
                } else if canAdd {
                    Button("Add this host", action: add).buttonStyle(.capstanPrimary)
                } else {
                    Button("Check again") { Task { await run() } }.buttonStyle(.capstanPrimary)
                    Button("Edit the host") { dismiss() }.buttonStyle(.capstanQuiet)
                }
            }
        }
        .sheet(item: Binding(
            get: { model.pendingTrust[hostId] },
            set: { if $0 == nil { model.cancelTrust(hostId: hostId) } }
        )) { pending in
            TrustSheet(
                pin: pending.pin, hostDisplay: display,
                onTrust: {
                    model.trust(hostId: hostId)
                    Task { await run() }
                },
                onCancel: { model.cancelTrust(hostId: hostId) })
        }
        .task { await run() }
        .onDisappear {
            if !added { model.discardCandidate(candidate) }
        }
    }

    private var failed: Bool {
        !running && checks.contains { $0.status == .failed }
    }

    private var canAdd: Bool {
        !running && check(.connect)?.status == .passed && !checks.contains { $0.status == .failed }
    }

    private func check(_ step: HostCheckStep) -> HostCheck? {
        checks.last { $0.step == step }
    }

    private func state(of step: HostCheckStep) -> CheckRow.State {
        guard let found = check(step) else { return running ? .running : .waiting }
        switch found.status {
        case .passed: return .passed
        case .warning: return .warning
        case .failed: return .failed
        }
    }

    private var trustState: CheckRow.State {
        if model.pendingTrust[hostId] != nil { return .failed }
        if check(.connect)?.status == .passed { return .passed }
        return running ? .running : .waiting
    }

    private func line(of step: HostCheckStep) -> String? {
        check(step).map { CheckStepText.make($0, destination: display).line }
    }

    private func run() async {
        guard !running else { return }
        running = true
        checks = []
        for await step in model.check(candidate.config) { checks.append(step) }
        running = false
        if canAdd { Haptics.play(.success) } else if failed { Haptics.play(.error) }
    }

    private func add() {
        added = true
        model.addHost(candidate)
        onAdded()
    }
}

extension PendingTrust: @retroactive Identifiable {
    public var id: UUID { hostId }
}

/// One step: a word and a glyph for its state, never colour alone, and the check's text.
struct CheckRow: View {
    enum State {
        case waiting, running, passed, warning, failed
    }

    let title: String
    let state: State
    let detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: title).font(TypeStyle.body)
                if let detail, state != .passed {
                    Text(verbatim: detail).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
            }
            Spacer()
            pill
        }
        .padding(Tokens.space[3])
        .frame(minHeight: 44)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private var pill: some View {
        switch state {
        case .waiting: StatePill(word: "Waiting", symbol: "circle", tone: .grey)
        case .running: StatePill(word: "Checking", symbol: "clock", tone: .blue)
        case .passed: StatePill(word: "Done", symbol: "checkmark", tone: .green)
        case .warning: StatePill(word: "Warning", symbol: "exclamationmark.triangle", tone: .yellow)
        case .failed: StatePill(word: "Failed", symbol: "xmark", tone: .red)
        }
    }
}

#Preview("Checks") {
    let seed = PreviewData.standard
    let host = seed.phoneHosts[0]
    let candidate = HostCandidate(config: seed.hosts[0], host: host)
    return NavigationStack { ChecksScreen(model: AppModel(preview: seed), candidate: candidate, onAdded: {}) }
}
