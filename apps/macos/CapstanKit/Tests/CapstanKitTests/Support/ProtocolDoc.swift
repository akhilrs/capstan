import Foundation

/// The protocol document, read from the repository.
enum ProtocolDoc {
    static func load() throws -> String {
        let url = RepoPaths.repoRoot.appendingPathComponent("docs/remote-protocol.md")
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The text of every ```json block with the number of the `## N.` section it is in.
    static func jsonBlocks() throws -> [(section: Int, text: String)] {
        var blocks: [(section: Int, text: String)] = []
        var section = 0
        var inJSON = false
        var inOther = false
        var current: [String] = []
        for line in try load().components(separatedBy: "\n") {
            if inJSON {
                if line.hasPrefix("```") {
                    blocks.append((section, current.joined(separator: "\n")))
                    current = []
                    inJSON = false
                } else {
                    current.append(line)
                }
            } else if inOther {
                if line.hasPrefix("```") { inOther = false }
            } else if line.hasPrefix("```json") {
                inJSON = true
            } else if line.hasPrefix("```") {
                inOther = true
            } else if line.hasPrefix("## "), let number = Int(line.dropFirst(3).prefix { $0.isNumber }) {
                section = number
            }
        }
        return blocks
    }

    /// The tables under the heading that starts with `heading`, each as its body rows of cells (the header and the
    /// separator row are left out, backticks are removed).
    static func tables(under heading: String) throws -> [[[String]]] {
        var tables: [[[String]]] = []
        var current: [[String]] = []
        var inSection = false
        var inFence = false
        func flush() {
            if current.count > 2 { tables.append(Array(current.dropFirst(2))) }
            current = []
        }
        for line in try load().components(separatedBy: "\n") {
            if line.hasPrefix("```") { inFence.toggle(); continue }
            if inFence { continue }
            if line.hasPrefix("## ") {
                flush()
                inSection = line.hasPrefix(heading)
                continue
            }
            guard inSection else { continue }
            if line.hasPrefix("|") {
                let cells = line.components(separatedBy: "|").dropFirst().dropLast()
                current.append(cells.map {
                    $0.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "`", with: "")
                })
            } else {
                flush()
            }
        }
        flush()
        return tables
    }

    /// All body rows of the tables under the heading.
    static func tableRows(under heading: String) throws -> [[String]] {
        try tables(under: heading).flatMap { $0 }
    }

    /// The leading integer of a cell such as `1000 ms`.
    static func number(_ cell: String) -> Int? {
        Int(cell.prefix { $0.isNumber })
    }
}
