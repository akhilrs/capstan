import CapstanAppCore
import SwiftUI

/// A menu of sounds (None, the system alert sounds, then the person's own) with a preview button.
/// `selection` nil means "same as the default" when `inherited` is given.
struct SoundPicker: View {
    let title: String
    @Binding var selection: SoundChoice?
    let catalog: SoundCatalog
    var inherited: SoundChoice?
    let player: SoundPlayer
    var fileCheck: (SoundChoice) -> Bool = SoundLibrary.usable

    private var effective: SoundChoice? { selection ?? inherited }

    private var state: PreviewState {
        effective.map { PreviewState.make($0, fileCheck: fileCheck) } ?? .disabled
    }

    var body: some View {
        HStack(spacing: Tokens.space[1]) {
            Picker(selection: $selection) {
                if inherited != nil {
                    Text("Same as default").tag(SoundChoice?.none)
                }
                Text("None").tag(Optional(SoundChoice.silent))
                Section("System sounds") {
                    ForEach(catalog.system) { entry in
                        Text(verbatim: entry.title).tag(Optional(entry.choice))
                    }
                }
                if !catalog.user.isEmpty {
                    Section("Your sounds") {
                        ForEach(catalog.user) { entry in
                            Text(verbatim: entry.title).tag(Optional(entry.choice))
                        }
                    }
                }
                if let missing = missingChoice {
                    Text(verbatim: "\(catalog.title(of: missing)) (not found)").tag(Optional(missing))
                }
            } label: {
                Text(verbatim: title)
            }
            Button {
                if let effective { player.toggle(effective) }
            } label: {
                Image(systemName: player.playing == effective && effective != nil ? "stop.fill" : "play.fill")
            }
            .disabled(state != .playable)
            .help("Preview the sound at the output volume")
            if state == .unavailable {
                Text("Sound unavailable").font(.system(size: 12)).foregroundStyle(Tokens.color(.red))
            }
        }
        .onDisappear { player.stop() }
    }

    /// A chosen sound that is not in the folders any more still needs a row, so the menu can show it.
    private var missingChoice: SoundChoice? {
        guard let selection, selection != .silent, !catalog.entries.contains(where: { $0.choice == selection }) else {
            return nil
        }
        return selection
    }
}

enum SoundPreviewFixtures {
    static let catalog = SoundCatalog(
        systemFiles: ["Basso.aiff", "Glass.aiff", "Ping.aiff", "Submarine.aiff"],
        userFiles: ["doorbell.wav", "low-chime.aiff"])
}

#Preview("Sound menu") {
    @Previewable @State var selection: SoundChoice? = .system("Glass.aiff")
    @Previewable @State var perKind: SoundChoice?
    VStack(alignment: .leading) {
        SoundPicker(
            title: "Default", selection: $selection, catalog: SoundPreviewFixtures.catalog, player: SoundPlayer(),
            fileCheck: { _ in true })
        SoundPicker(
            title: "Command proposals", selection: $perKind, catalog: SoundPreviewFixtures.catalog,
            inherited: .system("Glass.aiff"), player: SoundPlayer(), fileCheck: { _ in true })
    }
    .padding().frame(width: 480)
}

#Preview("Sound menu: gone sound") {
    @Previewable @State var selection: SoundChoice? = .user("gone.wav")
    SoundPicker(
        title: "Default", selection: $selection, catalog: SoundPreviewFixtures.catalog, player: SoundPlayer(),
        fileCheck: { _ in false })
    .padding().frame(width: 480)
}
