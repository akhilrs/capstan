import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Board 01, Add a host: host, user and port, the name in the app, optional extra project folders, with the
/// validation errors inline. Connect registers a candidate and opens its checks.
struct AddHostForm: View {
    @Bindable var model: AppModel
    /// Called when the host was added, or the person cancelled the sheet.
    let onDone: () -> Void

    @State private var hostname = ""
    @State private var user = ""
    @State private var port = "22"
    @State private var name = ""
    @State private var roots = ""
    @State private var cstanPath = "cstan"
    @State private var errors: [Field: String] = [:]
    @State private var formError: String?
    @State private var candidate: HostCandidate?
    @FocusState private var focus: Field?

    enum Field: Hashable {
        case hostname, user, port, name, roots, cstanPath
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                Text("Add a host").font(TypeStyle.largeTitle)
                if let formError { Banner(tone: .red, text: formError) }
                field("Host", text: $hostname, field: .hostname, prompt: "build-host.lan", mono: true)
                HStack(alignment: .top, spacing: Tokens.space[2]) {
                    field("User", text: $user, field: .user, prompt: "akhil", mono: true)
                    field("Port", text: $port, field: .port, prompt: "22", mono: true)
                        .frame(width: 96)
                        .keyboardType(.numberPad)
                }
                field("Name in the app", text: $name, field: .name, prompt: "build-host", mono: false)
                field("Extra project folders (optional)", text: $roots, field: .roots, prompt: "/srv/work/infra-tools", mono: true)
                Text("Capstan finds projects by itself. Add a folder only when it is outside the places discovery looks; one per line.")
                    .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                field("cstan on the host", text: $cstanPath, field: .cstanPath, prompt: "cstan", mono: true)
                if case .present(_, _, let fingerprint) = model.key {
                    Text(verbatim: "Uses this iPhone's key, " + fingerprint + ".")
                        .font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                }
            }
            .padding(.horizontal, Tokens.margin)
        }
        .background(Tokens.color(.ground))
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if !model.needsOnboarding {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onDone)
                }
            }
        }
        .safeAreaInset(edge: .bottom) {
            ActionBar {
                Button("Connect", action: connect)
                    .buttonStyle(.capstanPrimary)
                    .disabled(hostname.trimmingCharacters(in: .whitespaces).isEmpty || user.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .navigationDestination(item: $candidate) { made in
            ChecksScreen(model: model, candidate: made, onAdded: onDone)
        }
    }

    private func field(_ label: String, text: Binding<String>, field: Field, prompt: String, mono: Bool) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: label).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            TextField(text: text, prompt: Text(verbatim: prompt)) { Text(verbatim: label) }
                .font(mono ? TypeStyle.mono : TypeStyle.body)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focus, equals: field)
                .padding(Tokens.space[2])
                .frame(minHeight: 44)
                .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
                .overlay(
                    RoundedRectangle(cornerRadius: Tokens.radius.block)
                        .strokeBorder(errors[field] == nil ? Color.clear : Tokens.color(.red), lineWidth: 1))
            if let message = errors[field] {
                Label { Text(verbatim: message) } icon: { Image(systemName: "exclamationmark.circle") }
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.red))
            }
        }
    }

    private func connect() {
        errors = [:]
        formError = nil
        let host = hostname.trimmingCharacters(in: .whitespaces)
        let login = user.trimmingCharacters(in: .whitespaces)
        guard let number = Int(port.trimmingCharacters(in: .whitespaces)) else {
            errors[.port] = PhoneHostError.port.message
            return
        }
        for error in PhoneHost(id: UUID(), hostname: host, port: number, user: login).validate() {
            switch error {
            case .hostname: errors[.hostname] = error.message
            case .port: errors[.port] = error.message
            case .user: errors[.user] = error.message
            default: formError = error.message
            }
        }
        guard errors.isEmpty, formError == nil else { return }
        let folders = roots.split(whereSeparator: { $0 == "\n" || $0 == "," })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        let shownName = name.trimmingCharacters(in: .whitespaces)
        do {
            candidate = try model.prepareCandidate(
                displayName: shownName.isEmpty ? host : shownName, hostname: host, port: number, user: login,
                cstanPath: cstanPath.trimmingCharacters(in: .whitespaces), extraRoots: folders)
        } catch let error as PhoneHostError {
            formError = error.message
        } catch let error as HostConfigError {
            switch error {
            case .cstanPath: errors[.cstanPath] = error.message
            case .root: errors[.roots] = error.message
            default: formError = error.message
            }
        } catch {
            formError = "The host could not be added."
        }
    }
}

extension HostCandidate: @retroactive Hashable {
    public func hash(into hasher: inout Hasher) {
        hasher.combine(host.id)
    }
}

#Preview("Add a host") {
    NavigationStack { AddHostForm(model: AppModel(preview: PreviewData.standard), onDone: {}) }
}

#Preview("Add a host, dark") {
    NavigationStack { AddHostForm(model: AppModel(preview: PreviewData.standard), onDone: {}) }
        .preferredColorScheme(.dark)
}
