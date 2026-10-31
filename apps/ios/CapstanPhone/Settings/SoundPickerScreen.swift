import CapstanAppCore
import CapstanKit
import SwiftUI
import UniformTypeIdentifiers

/// The sound of a kind, or the default sound (board 14): Use default, None, the system alert tones and the person's
/// own tones, with a check mark on the chosen one. Tapping a tone chooses it and plays it once.
struct SoundPickerScreen: View {
    @Environment(AppModel.self) private var model
    /// Nil picks the default sound.
    let kind: WaitingKind?

    @State private var catalog = PhoneSounds.catalog()
    @State private var playing: SoundChoice?
    @State private var importing = false
    @State private var importError: String?

    var body: some View {
        List {
            if let kind {
                Section {
                    row(title: "Use default (" + catalog.title(of: model.settings.sounds.defaultSound) + ")",
                        selected: model.settings.sounds.perKind[kind.wireName] == nil, playingNow: false) {
                        choose(nil)
                    }
                }
            }
            Section {
                ForEach(catalog.entries.filter { $0.group != .user }) { entry in
                    row(title: entry.title, selected: isChosen(entry.choice), playingNow: playing == entry.choice) {
                        choose(entry.choice)
                    }
                }
            } header: {
                Text("Alert tones")
            }
            Section {
                ForEach(catalog.user) { entry in
                    row(title: entry.title, selected: isChosen(entry.choice), playingNow: playing == entry.choice) {
                        choose(entry.choice)
                    }
                }
                Button { importing = true } label: { Label("Add a tone…", systemImage: "plus") }
            } header: {
                Text("Your tones")
            } footer: {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    if let importError {
                        Text(verbatim: importError).foregroundStyle(Tokens.color(.red))
                    }
                    Text("The tones are read from the system when this screen opens. Your tones are AIFF, WAV or CAF files you add from Files. Tapping a tone plays it once.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(
            Text(verbatim: kind?.settingsName ?? "Default sound"))
        .navigationBarTitleDisplayMode(.large)
        .fileImporter(isPresented: $importing, allowedContentTypes: [.aiff, .wav, UTType(filenameExtension: "caf") ?? .audio]) { result in
            add(result)
        }
    }

    private func isChosen(_ choice: SoundChoice) -> Bool {
        if let kind { return model.settings.sounds.perKind[kind.wireName] == choice }
        return model.settings.sounds.defaultSound == choice
    }

    private func row(title: String, selected: Bool, playingNow: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(verbatim: title)
                    .foregroundStyle(selected ? Tokens.color(.accent) : Tokens.color(.label))
                Spacer()
                if playingNow {
                    Label("Playing", systemImage: "speaker.wave.2")
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.accent))
                }
                if selected {
                    Image(systemName: "checkmark").foregroundStyle(Tokens.color(.accent)).accessibilityHidden(true)
                }
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func choose(_ choice: SoundChoice?) {
        var settings = model.settings
        if let kind {
            settings.sounds.perKind[kind.wireName] = choice
        } else if let choice {
            settings.sounds.defaultSound = choice
        }
        model.updateSettings(settings)
        Haptics.play(.selection)
        let played = choice ?? settings.sounds.defaultSound
        guard PhoneSounds.play(played) else { return }
        playing = played
        Task {
            try? await Task.sleep(for: .seconds(2))
            if playing == played { playing = nil }
        }
    }

    private func add(_ result: Result<URL, Error>) {
        importError = nil
        switch result {
        case .success(let url):
            do {
                let name = try PhoneSounds.importTone(from: url)
                catalog = PhoneSounds.catalog()
                choose(.user(name))
            } catch PhoneSounds.ImportError.notASound {
                importError = "Only AIFF, WAV and CAF files can be tones."
            } catch {
                importError = "The file could not be copied."
            }
        case .failure:
            importError = "The file could not be opened."
        }
    }
}

#Preview("Sound for a kind") {
    NavigationStack { SoundPickerScreen(kind: .prompt) }
        .environment(AppModel(preview: PreviewData.standard))
}
