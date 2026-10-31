import CryptoKit
import Foundation

/// The binding of a message to the PM (section 11, Message to the PM): the request id `pm` and, as `--expect`, the
/// SHA-256 of `{"pm":"<agentId>"}` of the one active PM the person saw. Another PM taking over makes it `hash_mismatch`.
public enum PmBinding {
    public static let requestId = "pm"

    /// The active PM of a project: the one agent of kind `PM` in state `active`; nil with none or more than one.
    public static func activePm(_ project: ProjectModel) -> String? {
        let pms = project.agents.values.filter { $0.kind == "PM" && $0.state == "active" }
        return pms.count == 1 ? pms.first?.agentId : nil
    }

    /// The `--expect` for a PM: lowercase hex of the SHA-256 of the canonical JSON `{"pm":"<agentId>"}`.
    public static func hash(agentId: String) -> String {
        let json = "{\"pm\":" + jsonString(agentId) + "}"
        return SHA256.hash(data: Data(json.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    /// A JSON string as JavaScript's JSON.stringify writes it (agent ids hold no characters that need more).
    static func jsonString(_ text: String) -> String {
        var out = "\""
        for scalar in text.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }
}
