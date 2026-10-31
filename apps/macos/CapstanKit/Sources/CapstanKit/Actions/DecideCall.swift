import Foundation

/// What the person decided. Free text is checked and cleaned when the arguments are built.
public enum DecideVerb: Sendable, Equatable, Codable {
    case opDeny(note: String?)
    case opApprove
    case promptOption(Int)
    case promptEsc
    case designApprove(link: String, askedUser: String)
    case designChanges(askedUser: String, link: String?)
    case messageReply(String)
    case agentPause(reason: String?)
    case agentResume(reason: String?)
    case runPause(reason: String?)
    case runResume(reason: String?)
    /// Merges a waiting integration into the project's branch (section 11, Merge). Never pushes.
    case integrationMerge
    /// A free message from the person to the project's active PM (section 11, Message to the PM).
    case pmMessage(String)

    /// Denying and pausing cannot be taken back by the person with one more tap.
    public var isDestructive: Bool {
        switch self {
        case .opDeny, .runPause, .integrationMerge: return true
        default: return false
        }
    }

    /// The row of the protocol document's verbs table this verb belongs to.
    public var row: DecideVerbRow {
        switch self {
        case .opDeny: return DecideVerbRow(kind: "op", verb: "deny", outcome: "applied")
        case .opApprove: return DecideVerbRow(kind: "op", verb: "approve", outcome: "forwarded")
        case .promptOption: return DecideVerbRow(kind: "prompt", verb: "option", outcome: "forwarded")
        case .promptEsc: return DecideVerbRow(kind: "prompt", verb: "esc", outcome: "forwarded")
        case .designApprove: return DecideVerbRow(kind: "design", verb: "approve", outcome: "applied")
        case .designChanges: return DecideVerbRow(kind: "design", verb: "changes", outcome: "applied")
        case .messageReply: return DecideVerbRow(kind: "message", verb: "reply", outcome: "applied")
        case .agentPause: return DecideVerbRow(kind: "agent", verb: "pause", outcome: "applied")
        case .agentResume: return DecideVerbRow(kind: "agent", verb: "resume", outcome: "applied")
        case .runPause: return DecideVerbRow(kind: "run", verb: "pause", outcome: "applied")
        case .runResume: return DecideVerbRow(kind: "run", verb: "resume", outcome: "applied")
        case .integrationMerge: return DecideVerbRow(kind: "integration", verb: "merge", outcome: "applied")
        case .pmMessage: return DecideVerbRow(kind: "pm", verb: "message", outcome: "applied")
        }
    }
}

/// The reasons a request is refused here, before anything is sent.
public enum DecideRequestError: Error, Sendable, Equatable {
    case expect
    case client
    case nonce
    case optionNumber
    /// The named free text starts with `-`, which the host would read as a flag.
    case textDash(String)
    /// The named identifier starts with `-`, which the host would read as a flag.
    case idDash(String)
    /// The named free text is empty or longer than the cap.
    case text(String)

    /// A short sentence for a person.
    public var message: String {
        switch self {
        case .expect: return "The item hash must be 12 to 64 lowercase hex digits"
        case .client: return "The client label must be 1 to 64 letters, digits, spaces or . _ -"
        case .nonce: return "The request nonce is not valid"
        case .optionNumber: return "The option number is not valid"
        case .textDash(let name): return "The \(name) must not start with a dash"
        case .idDash(let name): return "The \(name) must not start with a dash"
        case .text(let name):
            return "The \(name) must be 1 to \(ProtocolLimits.reasonCap) bytes"
        }
    }
}

/// A fresh random token that makes a retried decision safe.
public enum Nonce {
    public static let length = 22
    static let alphabet = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-")

    static let leadingAlphabet = alphabet.filter { $0 != "-" }

    public static func make() -> String {
        var generator = SystemRandomNumberGenerator()
        var text = ""
        for index in 0..<length {
            // The first character is never `-`, which the host would read as a flag.
            let pool = index == 0 ? leadingAlphabet : alphabet
            // The pool is never empty; the fallback only satisfies the optional.
            text.append(pool.randomElement(using: &generator) ?? "A")
        }
        return text
    }

    static func isValid(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard (8...64).contains(bytes.count), text.first != "-" else { return false }
        return text.unicodeScalars.allSatisfy { alphabet.contains(Character($0)) }
    }
}

/// One decision to send to one project of one host.
public struct DecideRequest: Sendable, Equatable, Codable {
    public let hostId: UUID
    public let projectId: String
    public let requestId: String
    /// The content hash of the item as the client saw it: 12 to 64 lowercase hex digits.
    public let expect: String
    public let nonce: String
    public let client: String
    public let verb: DecideVerb

    public init(
        hostId: UUID, projectId: String, requestId: String, expect: String, nonce: String = Nonce.make(),
        client: String = "Mac", verb: DecideVerb
    ) {
        self.hostId = hostId
        self.projectId = projectId
        self.requestId = requestId
        self.expect = expect
        self.nonce = nonce
        self.client = client
        self.verb = verb
    }

    /// The arguments after `cstan`. Throws the local refusal when a value would not be accepted.
    public func remoteArguments() throws -> [String] {
        guard DecideRequest.isHash(expect) else { throw DecideRequestError.expect }
        guard DecideRequest.isClient(client) else { throw DecideRequestError.client }
        guard Nonce.isValid(nonce) else { throw DecideRequestError.nonce }
        guard !projectId.hasPrefix("-") else { throw DecideRequestError.idDash("project") }
        guard !requestId.hasPrefix("-") else { throw DecideRequestError.idDash("request") }
        let words = try verbWords()
        return ["decide", "--project", projectId, "--json", requestId, "--expect", expect, "--nonce", nonce,
                "--client", client] + words
    }

    /// The arguments of `/usr/bin/ssh` for this request.
    public func sshArguments(_ config: HostConfig) throws -> [String] {
        RemoteCommand.ssh(config, try remoteArguments())
    }

    /// The bare verb and its arguments: the kind is already named by the request id (`op:…`, `run`), so it is not repeated.
    private func verbWords() throws -> [String] {
        switch verb {
        case .opDeny(let note):
            return ["deny"] + (try DecideRequest.optionalText(note, "note"))
        case .opApprove:
            return ["approve"]
        case .promptOption(let number):
            guard number >= 1 else { throw DecideRequestError.optionNumber }
            return ["option", String(number)]
        case .promptEsc:
            return ["esc"]
        case .designApprove(let link, let askedUser):
            return ["approve", "--link", try DecideRequest.text(link, "link"),
                    "--asked-user", try DecideRequest.text(askedUser, "answer")]
        case .designChanges(let askedUser, let link):
            var words = ["changes", "--asked-user", try DecideRequest.text(askedUser, "answer")]
            if let link { words += ["--link", try DecideRequest.text(link, "link")] }
            return words
        case .messageReply(let reply):
            return ["reply", try DecideRequest.text(reply, "reply")]
        case .agentPause(let reason):
            return ["pause"] + (try DecideRequest.reasonWords(reason))
        case .agentResume(let reason):
            return ["resume"] + (try DecideRequest.reasonWords(reason))
        case .runPause(let reason):
            return ["pause"] + (try DecideRequest.reasonWords(reason))
        case .runResume(let reason):
            return ["resume"] + (try DecideRequest.reasonWords(reason))
        case .integrationMerge:
            return ["merge"]
        case .pmMessage(let text):
            return ["message", try DecideRequest.text(text, "message")]
        }
    }

    private static func reasonWords(_ reason: String?) throws -> [String] {
        guard let reason, !isBlank(reason) else { return [] }
        return ["--reason", try text(reason, "reason")]
    }

    private static func optionalText(_ value: String?, _ name: String) throws -> [String] {
        guard let value, !isBlank(value) else { return [] }
        return [try text(value, name)]
    }

    /// Empty or only spaces and control characters: optional text like this means no text.
    private static func isBlank(_ value: String) -> Bool {
        clean(value).trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// Cleans free text and checks its size. It is never cut: a text over the cap is refused.
    static func text(_ value: String, _ name: String) throws -> String {
        let cleaned = clean(value)
        let size = cleaned.utf8.count
        guard size >= 1, size <= ProtocolLimits.reasonCap,
              !cleaned.trimmingCharacters(in: .whitespaces).isEmpty else {
            throw DecideRequestError.text(name)
        }
        guard !cleaned.hasPrefix("-") else { throw DecideRequestError.textDash(name) }
        return cleaned
    }

    /// Control, bidirectional and separator characters become a space.
    static func clean(_ value: String) -> String {
        var scalars = String.UnicodeScalarView()
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x00...0x1F, 0x7F...0x9F, 0x061C, 0x200E, 0x200F, 0x2028...0x202E, 0x2066...0x2069, 0xFEFF:
                scalars.append(" ")
            default:
                scalars.append(scalar)
            }
        }
        return String(scalars)
    }

    static func isHash(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard (12...64).contains(bytes.count) else { return false }
        return bytes.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
    }

    static func isClient(_ text: String) -> Bool {
        let bytes = Array(text.utf8)
        guard (1...64).contains(bytes.count) else { return false }
        return bytes.allSatisfy { byte in
            (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || byte == 0x20 || byte == 0x2E || byte == 0x5F || byte == 0x2D
        }
    }
}
