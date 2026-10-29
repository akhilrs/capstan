import Foundation

struct ShParseError: Error, Equatable {
    let message: String
}

/// Splits a string into words the way a POSIX shell does for a plain command line: whitespace separates words,
/// single quotes keep everything, double quotes keep everything but let a backslash escape `"`, `\`, `$` and a
/// backtick, and a backslash outside quotes escapes the next character. Nothing is expanded.
enum ShParser {
    static func split(_ line: String) throws -> [String] {
        var words: [String] = []
        var current = ""
        var inWord = false
        var chars = Array(line.unicodeScalars)[...]
        while let c = chars.popFirst() {
            switch c {
            case " ", "\t", "\n":
                if inWord {
                    words.append(current)
                    current = ""
                    inWord = false
                }
            case "'":
                inWord = true
                var closed = false
                while let q = chars.popFirst() {
                    if q == "'" { closed = true; break }
                    current.unicodeScalars.append(q)
                }
                if !closed { throw ShParseError(message: "unterminated single quote") }
            case "\"":
                inWord = true
                var closed = false
                while let q = chars.popFirst() {
                    if q == "\"" { closed = true; break }
                    if q == "\\", let next = chars.first {
                        if next == "\"" || next == "\\" || next == "$" || next == "`" {
                            current.unicodeScalars.append(next)
                            chars.removeFirst()
                            continue
                        }
                        if next == "\n" {
                            chars.removeFirst()
                            continue
                        }
                    }
                    current.unicodeScalars.append(q)
                }
                if !closed { throw ShParseError(message: "unterminated double quote") }
            case "\\":
                guard let next = chars.popFirst() else { throw ShParseError(message: "trailing backslash") }
                if next == "\n" { continue }
                inWord = true
                current.unicodeScalars.append(next)
            default:
                inWord = true
                current.unicodeScalars.append(c)
            }
        }
        if inWord { words.append(current) }
        return words
    }
}
