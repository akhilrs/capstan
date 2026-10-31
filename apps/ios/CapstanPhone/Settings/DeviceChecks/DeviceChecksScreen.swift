import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import CryptoKit
import SwiftUI
import UIKit

/// Device checks (plan 6.3), a developer screen behind a long press on Settings → Version: what no unit test can
/// settle, run on a physical iPhone. Each check shows passed or failed and its text; host text stays plain. Copy
/// results gives the table to paste (check, expected, observed). The procedure is `apps/ios/docs/device-checks.md`.
struct DeviceChecksScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    @State private var results: [Check: Result] = [:]
    @State private var hostId: UUID?
    @State private var running: Check?
    @State private var steps: [HostCheck] = []
    @State private var linkWatch: Date?
    @State private var backgroundedAt: Date?
    @State private var awayFor: TimeInterval?
    @State private var activityResult: String?

    enum Check: Int, CaseIterable, Identifiable {
        case enclave = 1, faceID, localNetwork, sshLogin, stream, deadLink, background

        var id: Int { rawValue }

        var title: String {
            switch self {
            case .enclave: return "Secure Enclave key signs"
            case .faceID: return "Face ID unlocks"
            case .localNetwork: return "Local Network prompt, readable denial"
            case .sshLogin: return "SSH login with host-key trust"
            case .stream: return "cstan events for 10 s"
            case .deadLink: return "Dead link ends the channel"
            case .background: return "Face ID after 30 s and 90 s away"
            }
        }

        var expected: String {
            switch self {
            case .enclave: return "a signature that verifies"
            case .faceID: return "unlocked"
            case .localNetwork: return "prompt on first LAN connection; denial gives a readable error"
            case .sshLogin: return "all checks pass after Trust"
            case .stream: return "lines arrive; cursor advances"
            case .deadLink: return "no heartbeat after about 45 s (three 15 s server heartbeats), then the channel restarts"
            case .background: return "30 s: no Face ID; 90 s: Face ID"
            }
        }
    }

    struct Result: Equatable {
        let passed: Bool?
        let text: String
    }

    var body: some View {
        List {
            Section {
                Picker(selection: $hostId) {
                    Text("Choose a host").tag(UUID?.none)
                    ForEach(model.hosts, id: \.id) { host in
                        Text(verbatim: host.displayName).tag(UUID?.some(host.id))
                    }
                } label: {
                    Text("Host for checks 3 to 6")
                }
            } footer: {
                Text("Run these on a physical iPhone. The procedure is in apps/ios/docs/device-checks.md.")
            }
            ForEach(Check.allCases) { check in
                Section {
                    row(check)
                    action(check)
                } header: {
                    Text(verbatim: String(check.rawValue) + ". " + check.title)
                } footer: {
                    Text(verbatim: "Expected: " + check.expected)
                }
            }
            Section {
                Button("Start a sample: 7 waiting") {
                    let sample = GlanceSnapshot(
                        asOf: Date(), kindCounts: ["op": 2, "prompt": 2, "design": 1, "message": 1, "integration": 1],
                        projects: [], hostsReachable: 1, hostsTotal: 1, pollMs: model.settings.pollMs)
                    model.liveActivity.showSample(sample)
                    activityResult = model.liveActivity.lastResult
                }
                Button("Show the real counts") {
                    model.liveActivity.resumeReal()
                    model.publishGlance()
                    activityResult = model.liveActivity.lastResult
                }
                Button("End the Live Activity", role: .destructive) {
                    model.liveActivity.endAll()
                    activityResult = model.liveActivity.lastResult
                }
                if let activityResult {
                    Text(verbatim: activityResult).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
            } header: {
                Text("Live Activity")
            } footer: {
                Text("Lock the phone or open the Dynamic Island to see it. The real counts follow Settings → Live Activity.")
            }
            Section {
                Button { copyResults() } label: { Label("Copy results", systemImage: "doc.on.doc") }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Device checks")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { if hostId == nil { hostId = model.hosts.first?.id } }
        .onChange(of: scenePhase) { _, phase in phaseChanged(phase) }
        .onChange(of: hostId.flatMap { model.connection[$0] }) { _, state in linkStateChanged(state) }
    }

    private func row(_ check: Check) -> some View {
        let result = results[check]
        return HStack(alignment: .top, spacing: Tokens.space[2]) {
            Image(systemName: symbol(result, running: running == check))
                .foregroundStyle(color(result))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: word(result, running: running == check)).font(TypeStyle.subheadline.weight(.semibold))
                if let text = result?.text, !text.isEmpty {
                    Text(verbatim: TextBudget.displayPrefix(text, maxBytes: ProtocolLimits.reasonCap).text)
                        .font(TypeStyle.monoSmall)
                        .foregroundStyle(Tokens.color(.label2))
                        .textSelection(.enabled)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private func action(_ check: Check) -> some View {
        switch check {
        case .enclave:
            Button("Run") { runEnclave() }
        case .faceID:
            Button("Run") { Task { await runFaceID() } }
        case .localNetwork, .sshLogin:
            Button("Run the connection check") { Task { await runConnection(check) } }
                .disabled(hostId == nil || running != nil)
        case .stream:
            Button("Watch for 10 s") { Task { await runStream() } }
                .disabled(hostId == nil || running != nil)
        case .deadLink:
            if linkWatch == nil {
                Button("Start, then turn off Wi-Fi") { startDeadLink() }
                    .disabled(hostId == nil || !isLive)
            } else {
                Button("Stop watching", role: .destructive) {
                    linkWatch = nil
                    running = nil
                }
            }
        case .background:
            HStack {
                Button("Asked") { recordBackground(asked: true) }
                Spacer()
                Button("Not asked") { recordBackground(asked: false) }
            }
            .disabled(awayFor == nil)
        }
    }

    // MARK: Checks

    private func runEnclave() {
        guard SecureEnclave.isAvailable else {
            results[.enclave] = Result(passed: false, text: "No Secure Enclave on this device.")
            return
        }
        do {
            let key = try SecureEnclave.P256.Signing.PrivateKey()
            let payload = Data("capstan device check".utf8)
            let signature = try key.signature(for: payload)
            let verifies = key.publicKey.isValidSignature(signature, for: payload)
            var text = "Signed and " + (verifies ? "verified" : "did NOT verify") + " with a throwaway enclave key."
            if case .present(_, let publicLine, _) = model.key { text += "\nThis iPhone's key: " + publicLine }
            results[.enclave] = Result(passed: verifies, text: text)
        } catch {
            results[.enclave] = Result(passed: false, text: "The enclave refused: " + String(describing: error))
        }
    }

    private func runFaceID() async {
        running = .faceID
        let passed = await FaceIDGate().unlock(reason: "Device check: unlock with Face ID.")
        running = nil
        results[.faceID] = Result(passed: passed, text: passed ? "Unlocked." : "Cancelled or not available.")
    }

    private func runConnection(_ check: Check) async {
        guard let host = selectedHost else { return }
        running = check
        steps = []
        for await step in model.check(host) { steps.append(step) }
        running = nil
        let destination = model.phoneHost(host.id)?.display ?? host.displayName
        let lines = steps.map { step -> String in
            let text = CheckStepText.make(step, destination: destination)
            return text.title + ": " + text.line
        }
        let failed = steps.contains { $0.status == .failed }
        let problem = model.problems[host.id].map { "Problem: " + String(describing: $0) }
        results[check] = Result(passed: !failed, text: (lines + [problem].compactMap { $0 }).joined(separator: "\n"))
    }

    private func runStream() async {
        guard let id = hostId else { return }
        running = .stream
        var beats: Set<Date> = []
        let startHeads = heads(id)
        for _ in 0..<20 {
            if case .live(_, let last)? = model.connection[id] { beats.insert(last) }
            try? await Task.sleep(for: .milliseconds(500))
        }
        running = nil
        let endHeads = heads(id)
        let advanced = endHeads.filter { endHead in (startHeads[endHead.key] ?? 0) < endHead.value }.count
        let projects = model.hostModels[id]?.projects.count ?? 0
        let text = String(beats.count) + " distinct line times in 10 s · " + String(projects) + " projects · "
            + String(advanced) + " cursors advanced · highest seq " + String(endHeads.values.max() ?? 0)
        results[.stream] = Result(passed: !beats.isEmpty, text: text)
    }

    private func heads(_ id: UUID) -> [String: Int64] {
        var out: [String: Int64] = [:]
        for (key, project) in model.hostModels[id]?.projects ?? [:] { out[key] = project.head ?? 0 }
        return out
    }

    private func startDeadLink() {
        linkWatch = Date()
        running = .deadLink
        results[.deadLink] = Result(passed: nil, text: "Watching. Turn off Wi-Fi (or unplug the host's network) now.")
    }

    private func linkStateChanged(_ state: ConnectionState?) {
        guard let started = linkWatch else { return }
        let seconds = Int(Date().timeIntervalSince(started))
        switch state {
        case .noHeartbeat?:
            let before = results[.deadLink]?.text ?? ""
            results[.deadLink] = Result(passed: nil, text: before + "\nNo heartbeat after " + String(seconds) + " s.")
        case .connecting?, .reconnecting?, .stopped?:
            let before = results[.deadLink]?.text ?? ""
            results[.deadLink] = Result(passed: seconds <= 90, text: before + "\nChannel ended after " + String(seconds) + " s.")
            linkWatch = nil
            running = nil
        default:
            break
        }
    }

    private func phaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .background:
            backgroundedAt = Date()
        case .active:
            if let at = backgroundedAt {
                awayFor = Date().timeIntervalSince(at)
                backgroundedAt = nil
            }
        default:
            break
        }
    }

    private func recordBackground(asked: Bool) {
        guard let away = awayFor else { return }
        let seconds = Int(away)
        let expectedAsk = model.settings.faceID && away > 60
        let line = "Away " + String(seconds) + " s: Face ID " + (asked ? "asked" : "not asked")
            + " (expected " + (expectedAsk ? "asked" : "not asked") + ")."
        let before = results[.background]?.text
        let text = before.map { $0 + "\n" + line } ?? line
        let passed = (results[.background]?.passed ?? true) && asked == expectedAsk
        results[.background] = Result(passed: passed, text: text)
        awayFor = nil
    }

    // MARK: Helpers

    private var selectedHost: HostConfig? {
        hostId.flatMap { id in model.hosts.first { $0.id == id } }
    }

    private var isLive: Bool {
        if let id = hostId, case .live? = model.connection[id] { return true }
        return false
    }

    private func word(_ result: Result?, running: Bool) -> String {
        if running { return "Running" }
        switch result?.passed {
        case true?: return "Passed"
        case false?: return "Failed"
        case nil: return result == nil ? "Not run" : "In progress"
        }
    }

    private func symbol(_ result: Result?, running: Bool) -> String {
        if running { return "clock" }
        switch result?.passed {
        case true?: return "checkmark.circle.fill"
        case false?: return "xmark.circle.fill"
        case nil: return "circle.dashed"
        }
    }

    private func color(_ result: Result?) -> Color {
        switch result?.passed {
        case true?: return Tokens.color(.green)
        case false?: return Tokens.color(.red)
        case nil: return Tokens.color(.label2)
        }
    }

    /// The table to paste: check, expected, observed.
    private func copyResults() {
        var lines = ["| Check | Expected | Observed |", "|---|---|---|"]
        for check in Check.allCases {
            let result = results[check]
            let observed = word(result, running: false) + (result.map { ": " + $0.text } ?? "")
            let cell = observed.replacingOccurrences(of: "\n", with: "<br>").replacingOccurrences(of: "|", with: "/")
            lines.append("| " + String(check.rawValue) + ". " + check.title + " | " + check.expected + " | " + cell + " |")
        }
        let device = UIDevice.current
        lines.append("")
        lines.append(device.model + ", " + device.systemName + " " + device.systemVersion)
        UIPasteboard.general.string = lines.joined(separator: "\n")
        Haptics.play(.success)
    }
}

#Preview("Device checks") {
    NavigationStack { DeviceChecksScreen() }
        .environment(AppModel(preview: PreviewData.standard))
}
