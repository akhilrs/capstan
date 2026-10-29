import CapstanAppCore
import CapstanKit
import SwiftUI

/// The menu bar window: the newest waiting items, a way into the main window and Quit.
struct MenuBarContent: View {
    let model: AppModel

    var body: some View {
        MenuBarList(list: MenuList(rows: model.inbox().flatMap(\.rows)), now: Date(), open: { model.showMain(row: $0) })
    }
}

struct MenuBarList: View {
    let list: MenuList
    let now: Date
    let open: (InboxRowID?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            if list.shown.isEmpty {
                Text("Nothing is waiting.")
                    .font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
                    .padding(Tokens.space[1])
            }
            ForEach(list.shown) { row in
                Button { open(row.id) } label: { line(row) }
                    .buttonStyle(.plain)
            }
            if list.moreCount > 0 {
                Text(verbatim: "\(list.moreCount) more")
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                    .padding(.horizontal, Tokens.space[1])
            }
            Divider().padding(.vertical, Tokens.space[0])
            Button("Open Capstan") { open(nil) }
                .keyboardShortcut("o", modifiers: .command)
            Button("Quit Capstan") { NSApplication.shared.terminate(nil) }
                .keyboardShortcut("q", modifiers: .command)
        }
        .padding(Tokens.space[1])
        .frame(width: 320, alignment: .leading)
    }

    private func line(_ row: InboxRow) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
            Image(systemName: MenuList.symbol(row.kind)).frame(width: 16).foregroundStyle(Tokens.color(.label2))
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text(verbatim: MenuList.projectLine(row)).font(.system(size: 13, weight: .medium)).lineLimit(1)
                    Spacer(minLength: Tokens.space[0])
                    Text(verbatim: MenuList.age(since: row.since, now: now))
                        .font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                }
                Text(verbatim: MenuList.summaryLine(row))
                    .font(.system(size: 12)).foregroundStyle(Tokens.color(.label2)).lineLimit(1)
            }
        }
        .padding(Tokens.space[1])
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

struct MenuBarLabel: View {
    let model: AppModel
    let count: Int

    var body: some View {
        HStack(spacing: 2) {
            Image(systemName: "tray")
            if count > 0 {
                Text(verbatim: "\(count)")
            }
        }
        .registersWindowOpener(model)
    }
}

#Preview("Menu: 7 items") {
    MenuBarContent(model: AppModel(preview: PreviewData.standard))
}

#Preview("Menu: empty") {
    MenuBarList(list: MenuList(rows: []), now: Date(), open: { _ in })
}
