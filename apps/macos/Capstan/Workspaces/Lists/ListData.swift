import CapstanAppCore
import CapstanKit
import SwiftUI

/// CapstanAppCore's plan with its project; CapstanKit has a wire type of the same name.
typealias ListPlan = CapstanAppCore.PlanItem

extension AppModel {
    /// The agents of every ready project, by host name and then project name.
    func agentGroups() -> [AgentGroup] {
        AgentGroup.make(hosts: hosts, hostModels: hostModels)
    }

    /// The plans of every ready project, by host name and then project name.
    func planGroups() -> [PlanGroup] {
        PlanGroup.make(hosts: hosts, hostModels: hostModels)
    }
}

/// What the Agents and Plans screens and the toolbar read, for all projects or for the one in `scope`.
@MainActor
enum ListData {
    static func agentGroups(_ model: AppModel, scope: ProjectKey?) -> [AgentGroup] {
        let groups = model.agentGroups()
        guard let scope else { return groups }
        return groups.filter { $0.hostId == scope.hostId && $0.projectId == scope.projectId }
    }

    /// Every plan of the groups, newest first within a project, with how many integration items of it wait.
    static func planItems(_ groups: [PlanGroup]) -> [ListPlan] {
        groups.flatMap { group in
            group.rows.reversed().map { CapstanAppCore.PlanItem(group: group, row: $0, waiting: group.waiting($0)) }
        }
    }

    static func planGroups(_ model: AppModel, scope: ProjectKey?, pinned: [String]) -> [PlanGroup] {
        let ordered = PlanShow.ordered(model.planGroups(), pinned: pinned)
        guard let scope else { return ordered }
        return ordered.filter { $0.hostId == scope.hostId && $0.projectId == scope.projectId }
    }

    static func pinned() -> [String] {
        ProjectPins.list(UserDefaults.standard.string(forKey: ProjectPins.pinnedKey) ?? "")
    }
}

/// The agent or the project a row of the all-projects Agents list selected.
struct AgentPick: Hashable {
    let hostId: UUID
    let projectId: String
    /// Nil when the project's own row is selected.
    let agentId: String?
}

/// A small count bar for a project's agents; each segment is a share of the whole.
struct HealthBar: View {
    let counts: HealthCounts

    var body: some View {
        let colors = [
            Tokens.color(.accent), Tokens.color(.red), Tokens.color(.green), Tokens.color(.blue),
            Tokens.color(.label2).opacity(0.35),
        ]
        let parts = counts.barCounts
        let total = max(counts.total, 1)
        GeometryReader { geo in
            HStack(spacing: 0) {
                ForEach(0..<parts.count, id: \.self) { index in
                    Rectangle().fill(colors[index]).frame(width: geo.size.width * CGFloat(parts[index]) / CGFloat(total))
                }
            }
        }
        .frame(width: 64, height: 6)
        .background(Tokens.color(.fill2), in: Capsule())
        .clipShape(Capsule())
        .accessibilityHidden(true)
    }
}

/// The search field of the Agents and Plans toolbar. Esc clears it.
struct ListSearchField: View {
    @Binding var text: String
    let prompt: String
    var focus: FocusState<Bool>.Binding

    var body: some View {
        HStack(spacing: Tokens.space[0] + 2) {
            Image(systemName: "magnifyingglass").foregroundStyle(Tokens.color(.label2)).accessibilityHidden(true)
            TextField("", text: $text, prompt: Text(verbatim: prompt))
                .textFieldStyle(.plain)
                .focused(focus)
                .onExitCommand { text = "" }
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill") }
                    .buttonStyle(.plain)
                    .foregroundStyle(Tokens.color(.label2))
                    .accessibilityLabel("Clear search")
            }
        }
        .font(.system(size: 13))
        .padding(.horizontal, Tokens.space[2])
        .frame(width: 220, height: ToolbarMetrics.capsuleHeight)
        .contentShape(Capsule())
        .glassEffect(.regular, in: Capsule())
    }
}

/// A heading of the list: quiet text, or the accent with a warning mark for what needs you.
struct ListSectionTitle: View {
    let text: String
    var attention = false

    var body: some View {
        HStack(spacing: Tokens.space[1]) {
            if attention { Image(systemName: "exclamationmark.triangle").accessibilityHidden(true) }
            Text(verbatim: text)
        }
        .font(.system(size: 12, weight: .semibold))
        .foregroundStyle(attention ? Tokens.color(.accent) : Tokens.color(.label2))
        .padding(.horizontal, Tokens.space[2])
        .padding(.top, Tokens.space[2])
        .padding(.bottom, Tokens.space[0])
        .accessibilityAddTraits(.isHeader)
    }
}

/// The empty states of a list: a symbol, a title, one line and an optional action.
struct ListEmptyState: View {
    let symbol: String
    let title: String
    let detail: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Image(systemName: symbol).font(.system(size: 28)).foregroundStyle(Tokens.color(.label2)).accessibilityHidden(true)
            Text(verbatim: title).font(.system(size: 15, weight: .semibold))
            Text(verbatim: detail).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            if let actionTitle, let action {
                Button { action() } label: { Text(verbatim: actionTitle) }.buttonStyle(.capstanQuiet)
            }
        }
        .padding(Tokens.space[3] + Tokens.space[0])
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A row of the list: hover and selection are drawn here, so no system blue shows.
struct ListRowBackground: ViewModifier {
    let selected: Bool
    @State private var hovering = false

    func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: Tokens.radius.block)
                    .fill(selected ? Tokens.color(.accent).opacity(0.16) : (hovering ? Tokens.color(.fill2) : Color.clear)))
            .overlay(
                RoundedRectangle(cornerRadius: Tokens.radius.block)
                    .strokeBorder(Tokens.color(.accent), lineWidth: selected ? 1 : 0))
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
    }
}

extension View {
    func listRow(selected: Bool) -> some View { modifier(ListRowBackground(selected: selected)) }
}
