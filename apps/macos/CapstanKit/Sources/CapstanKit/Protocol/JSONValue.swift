import Foundation

/// A decoded JSON value. Integers that fit 64 bits stay integers; every other number is a double.
public indirect enum JSONValue: Codable, Sendable, Equatable {
    case null
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .int(value)
        } else if let value = try? container.decode(Double.self) {
            self = .double(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "not a JSON value"))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .int(let value): try container.encode(value)
        case .double(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }

    /// The member of an object, nil for another case or a missing member.
    public subscript(key: String) -> JSONValue? {
        if case .object(let members) = self { return members[key] }
        return nil
    }

    /// The element of an array, nil for another case or an index out of range.
    public subscript(index: Int) -> JSONValue? {
        if case .array(let elements) = self, elements.indices.contains(index) { return elements[index] }
        return nil
    }

    /// Overlays the members of `fields` on this object. Anything but two objects leaves this value as it is.
    public func merging(fields: JSONValue) -> JSONValue {
        guard case .object(var members) = self, case .object(let extra) = fields else { return self }
        for (key, value) in extra { members[key] = value }
        return .object(members)
    }
}

// Field access shared by the line, item and event decoders. A failure is `LineDecodeError.malformed`.
extension JSONValue {
    func members(_ what: String) throws -> [String: JSONValue] {
        guard case .object(let members) = self else { throw LineDecodeError.malformed("\(what) is not an object") }
        return members
    }

    func required(_ key: String) throws -> JSONValue {
        guard let value = self[key] else { throw LineDecodeError.malformed("missing \(key)") }
        return value
    }

    /// A member that may be missing or null.
    func optional(_ key: String) -> JSONValue? {
        guard let value = self[key], value != .null else { return nil }
        return value
    }

    func string(_ key: String) throws -> String {
        guard case .string(let value) = try required(key) else { throw LineDecodeError.malformed("\(key) is not a string") }
        return value
    }

    func optionalString(_ key: String) throws -> String? {
        guard let value = optional(key) else { return nil }
        guard case .string(let text) = value else { throw LineDecodeError.malformed("\(key) is not a string") }
        return text
    }

    func int64(_ key: String) throws -> Int64 {
        guard case .int(let value) = try required(key) else { throw LineDecodeError.malformed("\(key) is not an integer") }
        return value
    }

    func optionalInt64(_ key: String) throws -> Int64? {
        guard let value = optional(key) else { return nil }
        guard case .int(let number) = value else { throw LineDecodeError.malformed("\(key) is not an integer") }
        return number
    }

    func bool(_ key: String) throws -> Bool {
        guard case .bool(let value) = try required(key) else { throw LineDecodeError.malformed("\(key) is not a boolean") }
        return value
    }

    func optionalBool(_ key: String) throws -> Bool? {
        guard let value = optional(key) else { return nil }
        guard case .bool(let flag) = value else { throw LineDecodeError.malformed("\(key) is not a boolean") }
        return flag
    }
}
