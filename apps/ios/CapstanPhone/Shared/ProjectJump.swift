import CapstanAppCore
import SwiftUI

/// The projects the person pinned and the ones they opened last, kept on this iPhone only (UserDefaults through
/// `@AppStorage`). A project is named by `hostId/projectId`, the id of `AgentGroup` and `PlanGroup`. Pinned projects
/// come first on Agents and Plans.
enum ProjectPins {
    static let pinnedKey = "projects.pinned"
    static let recentKey = "projects.recent"
    /// How many recent projects the jump sheet keeps.
    static let recentCap = 5

    /// The ids in a stored value, in order.
    static func list(_ raw: String) -> [String] {
        raw.split(separator: "\n").map(String.init)
    }

    /// The stored value with `id` pinned if it was not, unpinned if it was.
    static func toggled(_ id: String, in raw: String) -> String {
        var ids = list(raw)
        if let index = ids.firstIndex(of: id) { ids.remove(at: index) } else { ids.append(id) }
        return ids.joined(separator: "\n")
    }

    /// The stored value with `id` first, at most `recentCap` long.
    static func touched(_ id: String, in raw: String) -> String {
        var ids = list(raw).filter { $0 != id }
        ids.insert(id, at: 0)
        return ids.prefix(recentCap).joined(separator: "\n")
    }
}

/// One project in the jump sheet: its name, host, a count ("7 agents", "34 plans") and its state in a word.
struct JumpEntry: Identifiable {
    let id: String
    let name: String
    let hostName: String
    let count: String
    let state: String
    let tone: Tone
}

/// The jump sheet: pinned projects, recent ones, then every project A to Z, with a filter field. Choosing one closes
/// the sheet and hands its id back; touching and holding one pins or unpins it.
struct ProjectJumpSheet: View {
    let entries: [JumpEntry]
    let pinned: [String]
    let recent: [String]
    let togglePin: (String) -> Void
    let pick: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var filter = ""

    var body: some View {
        let needle = filter.trimmingCharacters(in: .whitespaces)
        let sorted = entries.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let shown = needle.isEmpty ? sorted : sorted.filter {
            $0.name.localizedCaseInsensitiveContains(needle) || $0.hostName.localizedCaseInsensitiveContains(needle)
        }
        NavigationStack {
            List {
                if needle.isEmpty {
                    let pins = pinned.compactMap { id in entries.first { $0.id == id } }
                    if !pins.isEmpty {
                        Section("Pinned") { ForEach(pins) { row($0) } }
                    }
                    let recents = recent.compactMap { id in entries.first { $0.id == id } }
                    if !recents.isEmpty {
                        Section("Recent") { ForEach(recents) { row($0) } }
                    }
                }
                Section {
                    ForEach(shown) { row($0) }
                } header: {
                    Text(verbatim: "All projects · " + String(shown.count))
                } footer: {
                    Text("Touch and hold a project to pin or unpin it. Pinned projects come first on Agents and Plans.")
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Tokens.color(.ground))
            .searchable(text: $filter, placement: .navigationBarDrawer(displayMode: .always), prompt: Text("Filter projects"))
            .navigationTitle("Jump to project")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func row(_ entry: JumpEntry) -> some View {
        Button {
            pick(entry.id)
        } label: {
            HStack(spacing: Tokens.space[2]) {
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    HStack(spacing: Tokens.space[0]) {
                        Text(verbatim: entry.name).font(TypeStyle.headline).lineLimit(1)
                        if pinned.contains(entry.id) {
                            Image(systemName: "pin.fill")
                                .imageScale(.small)
                                .foregroundStyle(Tokens.color(.label2))
                                .accessibilityLabel("Pinned")
                        }
                    }
                    Text(verbatim: entry.hostName + " · " + entry.count)
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                }
                Spacer(minLength: Tokens.space[1])
                StatePill(word: entry.state, symbol: nil, tone: entry.tone).fixedSize()
            }
            .foregroundStyle(Tokens.color(.label))
            .contentShape(Rectangle())
        }
        .contextMenu {
            if pinned.contains(entry.id) {
                Button("Unpin", systemImage: "pin.slash") { togglePin(entry.id) }
            } else {
                Button("Pin", systemImage: "pin") { togglePin(entry.id) }
            }
        }
    }
}

/// A small bar of counts, one segment per kind, each as wide as its share. The words beside it say the same counts,
/// so the bar is hidden from VoiceOver.
struct CountBar: View {
    let parts: [(count: Int, color: Color)]
    var width: CGFloat = 48

    var body: some View {
        let shown = parts.filter { $0.count > 0 }
        let total = shown.reduce(0) { $0 + $1.count }
        let room = max(0, width - CGFloat(max(shown.count - 1, 0)) * 2)
        HStack(spacing: 2) {
            ForEach(Array(shown.enumerated()), id: \.offset) { _, part in
                Rectangle()
                    .fill(part.color)
                    .frame(width: total == 0 ? 0 : room * CGFloat(part.count) / CGFloat(total))
            }
        }
        .frame(width: width, height: 6, alignment: .leading)
        .background(Tokens.color(.fill2))
        .clipShape(RoundedRectangle(cornerRadius: 1))
        .accessibilityHidden(true)
    }
}
