import Foundation

public enum CursorError: Error, Equatable, Sendable {
    case invalid(String)
}

/// The position of one project in a cursor (document section 3).
public struct CursorEntry: Sendable, Equatable {
    /// The ledger epoch, 16 lowercase hex characters.
    public let e: String
    /// The last sequence read.
    public let s: Int64
    /// The sub-event of `s` last read; 0 when the field is omitted.
    public let k: Int64
    /// The history anchor of `s`, 16 lowercase hex characters.
    public let h: String
    /// The project root.
    public let r: String
    /// Unix seconds since the project was found unavailable; nil while it is available.
    public let d: Int64?

    public init(e: String, s: Int64, k: Int64 = 0, h: String, r: String, d: Int64? = nil) {
        self.e = e
        self.s = s
        self.k = k
        self.h = h
        self.r = r
        self.d = d
    }

    init(json: JSONValue) throws {
        let members = try json.members("cursor entry")
        let allowed: Set<String> = ["e", "s", "k", "h", "r", "d"]
        guard Set(members.keys).isSubset(of: allowed) else { throw CursorError.invalid("entry field") }
        do {
            e = try json.string("e")
            h = try json.string("h")
            r = try json.string("r")
            s = try json.int64("s")
            if let explicit = members["k"] {
                guard case .int(let value) = explicit, value > 0 else { throw CursorError.invalid("k") }
                k = value
            } else {
                k = 0
            }
            if let explicit = members["d"] {
                guard case .int(let value) = explicit else { throw CursorError.invalid("d") }
                d = value
            } else {
                d = nil
            }
        } catch let error as LineDecodeError {
            throw CursorError.invalid("\(error)")
        }
        guard CursorCodec.isHex16(e), CursorCodec.isHex16(h) else { throw CursorError.invalid("hex field") }
        guard s >= 0 else { throw CursorError.invalid("s") }
    }
}

public struct CursorPayload: Sendable, Equatable {
    /// The entry of each project, by project id.
    public let p: [String: CursorEntry]

    public init(p: [String: CursorEntry]) {
        self.p = p
    }

    public init(json: JSONValue) throws {
        let top = try Self.members(json)
        guard Set(top.keys) == ["p"], let projects = top["p"] else { throw CursorError.invalid("top-level key") }
        let entries = try Self.members(projects)
        var result: [String: CursorEntry] = [:]
        for (id, value) in entries { result[id] = try CursorEntry(json: value) }
        self.p = result
    }

    private static func members(_ json: JSONValue) throws -> [String: JSONValue] {
        guard case .object(let members) = json else { throw CursorError.invalid("not an object") }
        return members
    }
}

/// A cursor text `c1.…`.
public struct Cursor: Sendable, Equatable, Hashable {
    public static let prefix = "c1."

    public let text: String

    /// Nil unless the text has the `c1.` prefix followed by base64url characters.
    public init?(text: String) {
        guard text.hasPrefix(Cursor.prefix) else { return nil }
        let body = text.utf8.dropFirst(Cursor.prefix.utf8.count)
        guard !body.isEmpty, body.allSatisfy(CursorCodec.isBase64URL) else { return nil }
        self.text = text
    }

    public init(payload: CursorPayload) {
        self.text = CursorCodec.encode(payload)
    }

    /// Throws unless the text is exactly what the codec writes for the entries it holds.
    public func decoded() throws -> CursorPayload {
        let body = String(text.dropFirst(Cursor.prefix.count))
        guard let data = CursorCodec.base64URLDecode(body) else { throw CursorError.invalid("base64url") }
        let json: JSONValue
        do {
            json = try JSONDecoder().decode(JSONValue.self, from: data)
        } catch {
            throw CursorError.invalid("json")
        }
        let payload = try CursorPayload(json: json)
        guard CursorCodec.encode(payload) == text else { throw CursorError.invalid("not canonical") }
        return payload
    }

    /// The cursor without the given projects; nil when no entry is left.
    public func removing(_ ids: Set<String>) throws -> Cursor? {
        var entries = try decoded().p
        for id in ids { entries.removeValue(forKey: id) }
        if entries.isEmpty { return nil }
        return Cursor(payload: CursorPayload(p: entries))
    }
}

enum CursorCodec {
    static func isBase64URL(_ byte: UInt8) -> Bool {
        switch byte {
        case UInt8(ascii: "A")...UInt8(ascii: "Z"), UInt8(ascii: "a")...UInt8(ascii: "z"),
             UInt8(ascii: "0")...UInt8(ascii: "9"), UInt8(ascii: "-"), UInt8(ascii: "_"):
            return true
        default:
            return false
        }
    }

    static func isHex16(_ text: String) -> Bool {
        text.utf8.count == 16 && text.utf8.allSatisfy {
            ($0 >= UInt8(ascii: "0") && $0 <= UInt8(ascii: "9")) || ($0 >= UInt8(ascii: "a") && $0 <= UInt8(ascii: "f"))
        }
    }

    static func base64URLDecode(_ body: String) -> Data? {
        guard body.utf8.count % 4 != 1 else { return nil }
        var standard = body.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        standard += String(repeating: "=", count: (4 - standard.utf8.count % 4) % 4)
        return Data(base64Encoded: standard)
    }

    static func base64URLEncode(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func encode(_ payload: CursorPayload) -> String {
        Cursor.prefix + base64URLEncode(Data(json(payload).utf8))
    }

    /// The compact JSON the server writes: keys sorted by their UTF-8 bytes, no whitespace.
    static func json(_ payload: CursorPayload) -> String {
        let entries = sortedKeys(Array(payload.p.keys)).map { id -> String in
            guard let entry = payload.p[id] else { return "" }
            return quote(id) + ":" + json(entry)
        }
        return "{\"p\":{" + entries.joined(separator: ",") + "}}"
    }

    private static func json(_ entry: CursorEntry) -> String {
        var fields: [(String, String)] = [
            ("e", quote(entry.e)), ("h", quote(entry.h)), ("r", quote(entry.r)), ("s", String(entry.s)),
        ]
        if entry.k != 0 { fields.append(("k", String(entry.k))) }
        if let d = entry.d { fields.append(("d", String(d))) }
        fields.sort { Array($0.0.utf8).lexicographicallyPrecedes(Array($1.0.utf8)) }
        return "{" + fields.map { quote($0.0) + ":" + $0.1 }.joined(separator: ",") + "}"
    }

    private static func sortedKeys(_ keys: [String]) -> [String] {
        keys.sorted { Array($0.utf8).lexicographicallyPrecedes(Array($1.utf8)) }
    }

    /// Escapes only the quote, the backslash and U+0000 to U+001F, as the server's serializer does.
    private static func quote(_ text: String) -> String {
        var out = "\""
        for scalar in text.unicodeScalars {
            switch scalar.value {
            case 0x22: out += "\\\""
            case 0x5C: out += "\\\\"
            case 0x08: out += "\\b"
            case 0x0C: out += "\\f"
            case 0x0A: out += "\\n"
            case 0x0D: out += "\\r"
            case 0x09: out += "\\t"
            case 0x00...0x1F:
                let hex = String(scalar.value, radix: 16)
                out += "\\u" + String(repeating: "0", count: 4 - hex.count) + hex
            default:
                out.unicodeScalars.append(scalar)
            }
        }
        return out + "\""
    }
}
