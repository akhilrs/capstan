import CapstanAppCore
import CapstanKit
import SwiftUI

/// A project that is not ready (board 12): its root, its state as a word with a glyph, what the state means, and
/// how to fix it on the host. Check again reads the host again. The fix names no host command the record does not
/// know (R4): `no_key` says to set the project up with cstan on the host. A gone project can be removed.
struct ProjectStateScreen: View {
    @Environment(AppModel.self) private var model
    let hostId: UUID
    let projectId: String

    @State private var confirmRemove = false
    @State private var removeRefused = false

    var body: some View {
        if let project = model.project(hostId, projectId) {
            content(project)
        } else {
            ContentUnavailableView("This project is no longer listed", systemImage: "folder")
                .background(Tokens.color(.ground))
        }
    }

    private var hostName: String {
        model.hosts.first { $0.id == hostId }?.displayName ?? "the host"
    }

    private func content(_ project: ProjectModel) -> some View {
        let text = AvailabilityText(project.availability)
        let name = TextBudget.displayPrefix(project.displayName, maxBytes: 64).text
        return ScrollView {
            VStack(alignment: .leading, spacing: Tokens.space[3]) {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    HStack(spacing: Tokens.space[1]) {
                        Image(systemName: text.symbol).foregroundStyle(Tokens.color(.accent)).accessibilityHidden(true)
                        Text(verbatim: "Project · " + hostName).font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                    }
                    Text(verbatim: title(name, project.availability))
                        .font(TypeStyle.title2)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                }
                VStack(alignment: .leading, spacing: Tokens.space[2]) {
                    DetailFacts(facts: [DetailFact(label: "Root", value: project.root, mono: true)])
                    HStack(spacing: Tokens.space[2]) {
                        Text("State").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                        StatePill(word: text.word, symbol: text.symbol, tone: AvailabilityText.tone(project.availability))
                    }
                    .padding(.horizontal, Tokens.space[3])
                }
                Text(verbatim: text.meaning)
                    .font(TypeStyle.body)
                    .fixedSize(horizontal: false, vertical: true)
                if project.availability != .ready {
                    VStack(alignment: .leading, spacing: Tokens.space[1]) {
                        Text("Fix it on the host").font(TypeStyle.headline)
                        Text(verbatim: text.fix)
                            .font(TypeStyle.body)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(verbatim: "In a terminal on " + hostName + ", in " + project.root + ".")
                            .font(TypeStyle.subheadline)
                            .foregroundStyle(Tokens.color(.label2))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                if removeRefused {
                    Banner(tone: .yellow, text: "The project answers again, so it was not removed")
                }
                if project.availability == .gone {
                    Button(role: .destructive) { confirmRemove = true } label: {
                        Text("Remove project…")
                    }
                    .buttonStyle(.capstanDestructive)
                }
            }
            .padding(.horizontal, Tokens.margin)
            .padding(.vertical, Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.ground))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            Button { checkAgain() } label: { Text("Check again") }
                .buttonStyle(.capstanPrimary)
                .padding(.horizontal, Tokens.margin)
                .padding(.vertical, Tokens.space[2])
                .background(Tokens.color(.ground))
        }
        .sheet(isPresented: $confirmRemove) {
            ConfirmSheet(
                title: "Remove " + name + "?", detail: nil, noteLimit: nil, destructiveLabel: "Remove project",
                footnote: "The app stops listing this project. If the folder comes back, the host lists it again.",
                onConfirm: { _ in remove() })
        }
    }

    private func title(_ name: String, _ availability: ProjectAvailability) -> String {
        switch availability {
        case .ready: return name + " is ready"
        case .noKey: return name + " has no operator key"
        case .silent: return name + " is silent"
        case .refused: return name + " was refused"
        case .unsupported: return name + " runs a Capstan too old for this app"
        case .gone: return name + " is gone from the host"
        case .other: return name + " is in a state this app does not know"
        }
    }

    /// Asks the host again: a restart reads every project's state anew.
    private func checkAgain() {
        Haptics.play(.selection)
        model.restart(hostId: hostId)
    }

    private func remove() {
        Task {
            let result = await model.removeProject(hostId: hostId, projectId: projectId)
            if result == .removed {
                let path = model.navigation.path(model.navigation.tab).dropLast()
                model.navigation.show(model.navigation.tab, Array(path))
            } else {
                removeRefused = true
            }
        }
    }
}

#Preview("Project, no key") {
    NavigationStack { ProjectStateScreen(hostId: PreviewData.alphaId, projectId: "notes") }
        .environment(AppModel(preview: PreviewData.standard))
}

#Preview("Project, gone") {
    NavigationStack { ProjectStateScreen(hostId: PreviewData.betaId, projectId: "old") }
        .environment(AppModel(preview: PreviewData.standard))
}
