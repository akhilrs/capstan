import Foundation

/// Text that came from an agent, a message or a command on the host. It is shown as plain text only.
public struct UntrustedText: Codable, Sendable, Equatable {
    /// The text as the host sent it. Empty when `verified` is false.
    public let raw: String
    /// False when the host could not vouch for the text; there is no stand-in for it.
    public let verified: Bool
    /// True when the host cut the text to its cap.
    public let truncated: Bool

    public init(raw: String, verified: Bool, truncated: Bool) {
        self.raw = verified ? raw : ""
        self.verified = verified
        self.truncated = truncated
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            raw: try container.decode(String.self, forKey: .raw),
            verified: try container.decode(Bool.self, forKey: .verified),
            truncated: try container.decode(Bool.self, forKey: .truncated))
    }

    private enum CodingKeys: String, CodingKey {
        case raw, verified, truncated
    }

    /// `raw` with control, bidirectional-override, separator and byte-order-mark characters replaced by a space.
    public var displayPlain: String {
        var scalars = String.UnicodeScalarView()
        for scalar in raw.unicodeScalars {
            scalars.append(UntrustedText.isUnsafe(scalar) ? " " : scalar)
        }
        return String(scalars)
    }

    /// `displayPlain` that keeps line breaks: a message laid out in lines (a list, a small table) keeps its shape.
    /// `\r\n` and `\r` become `\n`; every other unsafe scalar is a space, as in `displayPlain`.
    public var displayLines: String {
        var scalars = String.UnicodeScalarView()
        let unified = raw.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        for scalar in unified.unicodeScalars {
            scalars.append(scalar == "\n" ? scalar : (UntrustedText.isUnsafe(scalar) ? " " : scalar))
        }
        return String(scalars)
    }

    private static func isUnsafe(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x00...0x1F, 0x7F...0x9F, 0x2028...0x202E, 0x2066...0x2069, 0xFEFF:
            return true
        default:
            return false
        }
    }
}
