import Foundation
@testable import CapstanKit

let testHash = "0123456789ab"
let testNonce = "AAAAAAAAAAAAAAAAAAAAAA"

func makeRequest(
    verb: DecideVerb, host: UUID = UUID(), expect: String = testHash, nonce: String = testNonce,
    client: String = "Mac", requestId: String = "op:p1", projectId: String = "proj-1"
) -> DecideRequest {
    DecideRequest(
        hostId: host, projectId: projectId, requestId: requestId, expect: expect, nonce: nonce, client: client,
        verb: verb)
}

func resultLine(
    outcome: String = "applied", isRepeat: Bool = false, replaces: String? = nil, nonce: String = testNonce
) -> String {
    var fields = [
        "\"v\":1", "\"decisionId\":\"d1\"", "\"requestId\":\"op:p1\"", "\"nonce\":\"\(nonce)\"",
        "\"outcome\":\"\(outcome)\"", "\"repeat\":\(isRepeat)", "\"kind\":\"op\"", "\"verb\":\"deny\"",
        "\"contentHash\":\"\(testHash)\"", "\"eventSeq\":7",
    ]
    if outcome == "forwarded" { fields.append("\"messageId\":\"m2\"") }
    if let replaces { fields.append("\"replaces\":\"\(replaces)\"") }
    return "{" + fields.joined(separator: ",") + "}\n"
}

func refusalLine(_ code: String, message: String = "no") -> String {
    "{\"v\":1,\"error\":{\"code\":\"\(code)\",\"message\":\"\(message)\"}}\n"
}

func tempStoreURL() -> URL {
    FileManager.default.temporaryDirectory
        .appendingPathComponent("capstan-pending-\(UUID().uuidString)", isDirectory: true)
        .appendingPathComponent("pending.json")
}

func waitingItem(kind: String = "prompt", needsTerminal: Bool = false) throws -> WaitingItem {
    let json = JSONValue.object([
        "key": .string("k"), "requestId": .string("\(kind):1"), "kind": .string(kind), "since": .string("t"),
        "summary": .string("s"), "contentHash": .string(testHash), "needsTerminal": .bool(needsTerminal),
    ])
    return try WaitingItem(json: json)
}

func promptOption(_ number: Int64 = 1, accepts: Bool = false, widens: Bool = false) -> PromptOption {
    PromptOption(number: number, text: hostText("o"), acceptsText: accepts, widensPermissions: widens)
}
