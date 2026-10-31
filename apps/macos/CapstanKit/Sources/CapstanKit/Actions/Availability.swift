import Foundation

/// Whether a verb can be offered for an item.
public enum DecideAvailability: Sendable, Equatable {
    case available
    /// Shown, but the person has to do it in a terminal; the text says why.
    case terminalOnly(String)
    /// Not shown; the text says why.
    case hidden(String)
}

/// The rules of the client for one verb. The host decides again when the request arrives.
public func availability(
    _ verb: DecideVerb, capabilities: HostCapabilities, item: WaitingItem?, option: PromptOption?
) -> DecideAvailability {
    guard capabilities.decide else { return .hidden("The host's cstan cannot take decisions") }
    if let item, let expected = waitingKind(of: verb), item.kind != expected {
        return .hidden("The item is not of the kind this verb answers")
    }
    // Only a built integration can be merged; a conflicted one has nothing to merge.
    if verb == .integrationMerge, let item, item.integrationState != "merged" {
        return .hidden("Only an integration that is built can be merged")
    }
    if case .promptOption = verb {
        guard let option else { return .hidden("No such option") }
        if option.widensPermissions { return .terminalOnly("This option widens permissions") }
        if option.acceptsText { return .terminalOnly("This option takes typed text") }
    }
    // An item that needs a terminal allows only esc.
    if item?.needsTerminal == true, verb != .promptEsc {
        return .terminalOnly("This item can only be answered in a terminal")
    }
    return .available
}

private func waitingKind(of verb: DecideVerb) -> WaitingKind? {
    switch verb {
    case .opDeny, .opApprove: return .op
    case .promptOption, .promptEsc: return .prompt
    case .designApprove, .designChanges: return .design
    case .messageReply: return .message
    case .agentPause, .agentResume, .runPause, .runResume: return nil
    case .integrationMerge: return .integration
    }
}
