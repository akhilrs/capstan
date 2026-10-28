import AppKit
import SwiftUI

/// Chooses a file with the open panel and keeps its path only; the file is never read.
struct FilePathPicker: View {
    @Binding var path: String?

    var body: some View {
        HStack(spacing: Tokens.space[1]) {
            Text(verbatim: path ?? "No file chosen")
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(path == nil ? Tokens.color(.label2) : Tokens.color(.label))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Choose…") { choose() }
            if path != nil {
                Button("Clear") { path = nil }
            }
        }
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        if panel.runModal() == .OK, let url = panel.url {
            path = url.path
        }
    }
}

#Preview("FilePathPicker") {
    @Previewable @State var path: String? = "~/.ssh/id_ed25519"
    @Previewable @State var empty: String?
    VStack {
        FilePathPicker(path: $path)
        FilePathPicker(path: $empty)
    }
    .padding()
    .frame(width: 420)
}
