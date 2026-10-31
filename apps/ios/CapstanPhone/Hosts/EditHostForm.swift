import CapstanAppCore
import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// Edit a host: the name in the app, where it is, the cstan path, extra project folders and the poll interval,
/// with the validation errors inline. A new address forgets the host key, so the next connection asks to trust it.
struct EditHostForm: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let config: HostConfig
    let phone: PhoneHost

    @State private var name: String
    @State private var hostname: String
    @State private var user: String
    @State private var port: String
    @State private var cstanPath: String
    @State private var roots: String
    @State private var pollMs: String
    @State private var errors: [Field: String] = [:]
    @State private var formError: String?

    enum Field: Hashable {
        case name, hostname, user, port, cstanPath, roots, pollMs
    }

    init(config: HostConfig, phone: PhoneHost) {
        self.config = config
        self.phone = phone
        _name = State(initialValue: config.displayName)
        _hostname = State(initialValue: phone.hostname)
        _user = State(initialValue: phone.user)
        _port = State(initialValue: String(phone.port))
        _cstanPath = State(initialValue: config.cstanPath)
        _roots = State(initialValue: config.extraRoots.joined(separator: "\n"))
        _pollMs = State(initialValue: String(config.pollMs))
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[3]) {
                    if let formError { Banner(tone: .red, text: formError) }
                    field("Name in the app", text: $name, field: .name, mono: false)
                    field("Host", text: $hostname, field: .hostname, mono: true)
                    HStack(alignment: .top, spacing: Tokens.space[2]) {
                        field("User", text: $user, field: .user, mono: true)
                        field("Port", text: $port, field: .port, mono: true)
                            .frame(width: 96)
                            .keyboardType(.numberPad)
                    }
                    field("cstan on the host", text: $cstanPath, field: .cstanPath, mono: true)
                    field("Extra project folders, one per line", text: $roots, field: .roots, mono: true, lines: true)
                    field("Poll interval (ms)", text: $pollMs, field: .pollMs, mono: true)
                        .keyboardType(.numberPad)
                    Text("Changing the host, user or port forgets the trusted host key: the next connection asks you to trust it again.")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                }
                .padding(Tokens.margin)
            }
            .background(Tokens.color(.ground))
            .navigationTitle("Edit host")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { save() } }
            }
        }
    }

    private func field(_ label: String, text: Binding<String>, field: Field, mono: Bool, lines: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: label).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            Group {
                if lines {
                    TextField(text: text, axis: .vertical) { Text(verbatim: label) }.lineLimit(2...6)
                } else {
                    TextField(text: text) { Text(verbatim: label) }
                }
            }
            .font(mono ? TypeStyle.mono : TypeStyle.body)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
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

    private func save() {
        errors = [:]
        formError = nil
        guard let number = Int(port.trimmingCharacters(in: .whitespaces)) else {
            errors[.port] = PhoneHostError.port.message
            return
        }
        guard let interval = Int(pollMs.trimmingCharacters(in: .whitespaces)) else {
            errors[.pollMs] = HostConfigError.pollMs.message
            return
        }
        let host = PhoneHost(
            id: phone.id, hostname: hostname.trimmingCharacters(in: .whitespaces), port: number,
            user: user.trimmingCharacters(in: .whitespaces))
        for error in host.validate() {
            switch error {
            case .hostname: errors[.hostname] = error.message
            case .port: errors[.port] = error.message
            case .user: errors[.user] = error.message
            default: formError = error.message
            }
        }
        var updated = config
        let shownName = name.trimmingCharacters(in: .whitespaces)
        updated.displayName = shownName.isEmpty ? host.hostname : shownName
        updated.cstanPath = cstanPath.trimmingCharacters(in: .whitespaces)
        updated.extraRoots = roots.split(whereSeparator: { $0 == "\n" || $0 == "," })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        updated.pollMs = interval
        for error in updated.validate() {
            switch error {
            case .cstanPath: errors[.cstanPath] = error.message
            case .root: errors[.roots] = error.message
            case .pollMs: errors[.pollMs] = error.message
            default: formError = error.message
            }
        }
        if let message = HostLimits.pollError(updated), errors[.pollMs] == nil { errors[.pollMs] = message }
        guard errors.isEmpty, formError == nil else { return }
        do {
            try model.updateHost(updated, host: host)
            dismiss()
        } catch let error as PhoneHostError {
            formError = error.message
        } catch {
            formError = "The host could not be saved."
        }
    }
}

/// The sheets any screen can ask for through `navigation.sheet`: edit a host, trust a host key, reset the pins.
extension View {
    func hostSheets(_ model: AppModel) -> some View {
        modifier(HostSheets(model: model))
    }
}

private struct HostSheets: ViewModifier {
    @Bindable var model: AppModel

    func body(content: Content) -> some View {
        content
            .sheet(isPresented: binding { if case .editHost = $0 { return true } else { return false } }) {
                if case .editHost(let id)? = model.navigation.sheet,
                    let config = model.hosts.first(where: { $0.id == id }), let phone = model.phoneHost(id)
                {
                    EditHostForm(config: config, phone: phone).environment(model)
                }
            }
            .sheet(isPresented: binding { if case .trust = $0 { return true } else { return false } }) {
                if case .trust(let id)? = model.navigation.sheet, let pending = model.pendingTrust[id] {
                    TrustSheet(
                        pin: pending.pin, hostDisplay: model.phoneHost(id)?.display ?? "the host",
                        onTrust: {
                            model.trust(hostId: id)
                            model.navigation.sheet = nil
                        },
                        onCancel: {
                            model.cancelTrust(hostId: id)
                            model.navigation.sheet = nil
                        })
                }
            }
            .sheet(isPresented: binding { $0 == .resetPins }) {
                ConfirmSheet(
                    title: "Reset all host keys?", detail: nil, noteLimit: nil, destructiveLabel: "Reset all host keys",
                    footnote: "The app forgets every trusted host key. Each host asks you to trust its key again on the next connection; compare the fingerprint on the host before you trust it.",
                    onConfirm: { _ in model.resetAllPins() })
            }
    }

    private func binding(_ matches: @escaping (PhoneSheet) -> Bool) -> Binding<Bool> {
        Binding(
            get: { model.navigation.sheet.map(matches) ?? false },
            set: { shown in
                if !shown, let sheet = model.navigation.sheet, matches(sheet) { model.navigation.sheet = nil }
            })
    }
}
