import CapstanAppCore
import CapstanKit
import SwiftUI

/// Adds a host or edits one: the fields, every validation error beside its field and a live connection check.
/// The check of an existing host runs with the edited config over its own ssh; the live session is not touched.
struct AddHostSheet: View {
    let model: AppModel
    let editing: HostConfig?
    let onDone: (HostConfig?) -> Void

    @State private var hostId: UUID
    @State private var displayName: String
    @State private var destination: String
    @State private var cstanPath: String
    @State private var rootsText: String
    @State private var pollText: String
    @State private var identityFile: String?
    @State private var touched: Bool
    @State private var checks: [HostCheck]
    @State private var running: Bool
    @State private var checkTask: Task<Void, Never>?

    private static let steps: [HostCheckStep] = [.connect, .capstan, .projects, .firstSnapshot]

    init(model: AppModel, editing: HostConfig?, onDone: @escaping (HostConfig?) -> Void) {
        self.init(model: model, editing: editing, onDone: onDone, seedChecks: [], seedRunning: false)
    }

    /// For previews: starts with these check results, and with the next step running when `seedRunning`.
    init(
        model: AppModel, editing: HostConfig?, onDone: @escaping (HostConfig?) -> Void, seedChecks: [HostCheck],
        seedRunning: Bool
    ) {
        self.model = model
        self.editing = editing
        self.onDone = onDone
        _hostId = State(initialValue: editing?.id ?? UUID())
        _displayName = State(initialValue: editing?.displayName ?? "")
        _destination = State(initialValue: editing?.destination ?? "")
        _cstanPath = State(initialValue: editing?.cstanPath ?? "cstan")
        _rootsText = State(initialValue: (editing?.extraRoots ?? []).joined(separator: "\n"))
        _pollText = State(initialValue: String(editing?.pollMs ?? 2000))
        _identityFile = State(initialValue: editing?.identityFile)
        _touched = State(initialValue: editing != nil)
        _checks = State(initialValue: seedChecks)
        _running = State(initialValue: seedRunning)
        _checkTask = State(initialValue: nil)
    }

    /// The config the form describes now; a poll interval that is not a number is 0, which validation refuses.
    private var config: HostConfig {
        let roots = rootsText.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        return HostConfig(
            id: hostId, displayName: displayName.trimmingCharacters(in: .whitespaces),
            destination: destination.trimmingCharacters(in: .whitespaces),
            cstanPath: cstanPath.trimmingCharacters(in: .whitespaces), extraRoots: roots,
            pollMs: Int(pollText.trimmingCharacters(in: .whitespaces)) ?? 0, identityFile: identityFile,
            enabled: editing?.enabled ?? true)
    }

    private var errors: [HostConfigError] { config.validate() }
    private var pollError: String? { HostLimits.pollError(config) }
    private var valid: Bool { errors.isEmpty && pollError == nil }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            Text(verbatim: editing == nil ? "Add host" : "Edit host").font(.system(size: 17, weight: .semibold))
            Form {
                field("Name", "Shown in the sidebar; the destination when empty", text: $displayName, error: nil)
                field(
                    "SSH host", "A host name, user@host or an ssh config alias", text: $destination,
                    error: message(.destination))
                field("cstan path", "On the host", text: $cstanPath, error: message(.cstanPath))
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    LabeledContent("Extra folders") {
                        TextField("One absolute path per line", text: $rootsText, axis: .vertical)
                            .lineLimit(2...5).labelsHidden()
                    }
                    ForEach(rootErrors, id: \.self) { line in inline(line) }
                }
                field("Poll interval (ms)", "How often the host is asked", text: $pollText, error: pollError)
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    LabeledContent("Key file") { FilePathPicker(path: $identityFile) }
                    if let line = message(.identityFile) { inline(line) }
                }
            }
            .onChange(of: config) { _, _ in
                touched = true
                cancelCheck()
                checks = []
            }
            checkSection
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { finish(nil) }.keyboardShortcut(.cancelAction)
                Button { finish(config) } label: { Text(verbatim: editing == nil ? "Add" : "Save") }
                    .keyboardShortcut(.defaultAction).disabled(!valid)
            }
        }
        .padding(Tokens.space[4])
        .frame(minWidth: 480)
        .onDisappear { cancelCheck() }
    }

    private func field(_ title: String, _ prompt: String, text: Binding<String>, error: String?) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            LabeledContent(title) { TextField("", text: text, prompt: Text(verbatim: prompt)).labelsHidden() }
            if let error { inline(error) }
        }
    }

    private func inline(_ line: String) -> some View {
        Text(verbatim: line).font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
    }

    private func message(_ error: HostConfigError) -> String? {
        guard touched else { return nil }
        return errors.first { sameCase($0, error) }?.message
    }

    private var rootErrors: [String] {
        guard touched else { return [] }
        return errors.compactMap { if case .root = $0 { return $0.message } else { return nil } }
    }

    private func sameCase(_ a: HostConfigError, _ b: HostConfigError) -> Bool {
        switch (a, b) {
        case (.destination, .destination), (.cstanPath, .cstanPath), (.identityFile, .identityFile): return true
        default: return false
        }
    }

    // MARK: Check

    @ViewBuilder
    private var checkSection: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack {
                Text("Connection check").font(.system(size: 13, weight: .semibold))
                Spacer()
                Button("Check connection") { runCheck() }.disabled(!valid || running)
            }
            if running || !checks.isEmpty {
                ForEach(Self.steps, id: \.self) { step in
                    row(step)
                }
            }
        }
    }

    private func row(_ step: HostCheckStep) -> some View {
        let destination = config.destination
        if let found = checks.last(where: { $0.step == step }) {
            let text = CheckStepText.make(found, destination: destination)
            return CheckStepRow(
                title: text.title, state: state(found.status),
                detail: UntrustedText(raw: text.line, verified: true, truncated: false))
        }
        let title = CheckStepText.make(HostCheck(step: step, status: .passed, text: ""), destination: destination).title
        let next = Self.steps.first { s in !checks.contains { $0.step == s } }
        return CheckStepRow(title: title, state: running && next == step ? .running : .pending)
    }

    private func state(_ status: HostCheckStatus) -> CheckStepState {
        switch status {
        case .passed: return .passed
        case .warning: return .warning
        case .failed: return .failed
        }
    }

    private func runCheck() {
        cancelCheck()
        checks = []
        running = true
        let stream = model.check(config)
        checkTask = Task {
            for await result in stream {
                if Task.isCancelled { return }
                checks.append(result)
            }
            if !Task.isCancelled { running = false }
        }
    }

    private func cancelCheck() {
        checkTask?.cancel()
        checkTask = nil
        running = false
    }

    private func finish(_ result: HostConfig?) {
        cancelCheck()
        onDone(result)
    }
}

#Preview("Add host: empty") {
    AddHostSheet(model: AppModel(preview: PreviewSeed()), editing: nil) { _ in }
}

#Preview("Edit host: every check result") {
    ScrollView {
        VStack(spacing: Tokens.space[3]) {
            ForEach(Array(HostsPreviewFixtures.checkRuns.enumerated()), id: \.offset) { _, run in
                AddHostSheet(
                    model: AppModel(preview: PreviewData.standard),
                    editing: PreviewData.standard.hosts.first, onDone: { _ in }, seedChecks: run.checks,
                    seedRunning: run.name == "in progress")
            }
        }
    }
    .frame(width: 520, height: 800)
}

#Preview("Edit host: every validation error") {
    ScrollView {
        VStack(spacing: Tokens.space[3]) {
            ForEach(Array(HostsPreviewFixtures.invalidConfigs.enumerated()), id: \.offset) { _, entry in
                AddHostSheet(model: AppModel(preview: PreviewSeed()), editing: entry.config) { _ in }
            }
        }
    }
    .frame(width: 520, height: 800)
}
