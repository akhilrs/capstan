import Foundation
@testable import CapstanKit

/// Wire lines and values shared by the stream tests.
enum Wire {
    static let hello = "{\"v\":1,\"kind\":\"hello\",\"version\":\"0.1\",\"host\":\"box\",\"projects\":2}\n"
    static let heartbeat = "{\"v\":1,\"kind\":\"heartbeat\",\"ts\":\"2026-01-01T00:00:00Z\"}\n"
    static let future = "{\"v\":1,\"kind\":\"somethingNew\",\"x\":1}\n"
    static let snapshotEnd = "{\"v\":1,\"kind\":\"snapshotEnd\",\"projectId\":\"p1\",\"head\":3}\n"

    static func project(_ id: String, state: String) -> String {
        "{\"v\":1,\"kind\":\"project\",\"projectId\":\"\(id)\",\"root\":\"/work/\(id)\",\"state\":\"\(state)\"}\n"
    }

    static func snapshot(_ project: String, key: String) -> String {
        "{\"v\":1,\"kind\":\"snapshot\",\"projectId\":\"\(project)\",\"section\":\"waiting\",\"key\":\"\(key)\","
            + "\"item\":{\"key\":\"\(key)\",\"requestId\":\"\(key)\",\"kind\":\"op\",\"since\":\"2026-10-01T09:30:00.000Z\","
            + "\"summary\":\"run ls\",\"contentHash\":\"h\",\"needsTerminal\":false}}\n"
    }

    static func cursor(_ cursor: Cursor) -> String {
        "{\"v\":1,\"kind\":\"cursor\",\"cursor\":\"\(cursor.text)\"}\n"
    }

    static func error(_ code: String, _ message: String = "text") -> String {
        "{\"v\":1,\"kind\":\"error\",\"code\":\"\(code)\",\"message\":\"\(message)\"}\n"
    }
}

func makeCursor(_ ids: [String], sequence: Int64 = 5) -> Cursor {
    var entries: [String: CursorEntry] = [:]
    for id in ids {
        entries[id] = CursorEntry(e: "0123456789abcdef", s: sequence, h: "fedcba9876543210", r: "/work/\(id)")
    }
    return Cursor(payload: CursorPayload(p: entries))
}

func hostText(_ raw: String) -> UntrustedText {
    UntrustedText(raw: raw, verified: true, truncated: false)
}

func makeConfig(
    destination: String = "user@box", pollMs: Int = 2000, roots: [String] = [], identityFile: String? = nil,
    cstanPath: String = "cstan"
) -> HostConfig {
    HostConfig(
        displayName: "Box", destination: destination, cstanPath: cstanPath, extraRoots: roots, pollMs: pollMs,
        identityFile: identityFile)
}

/// The value after a flag in a split remote command line.
func flagValue(_ flag: String, in words: [String]) -> String? {
    guard let index = words.firstIndex(of: flag), index + 1 < words.count else { return nil }
    return words[index + 1]
}
