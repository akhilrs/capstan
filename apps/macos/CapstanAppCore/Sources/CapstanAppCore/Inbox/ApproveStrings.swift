/// The fixed strings of the approve design, one for each key of docs/design/plan-153-design-approve/strings.json.
/// scripts/check-approve-strings.py checks this file against that one. Placeholders: {n} {total} {windowId} {time}.
public enum ApproveStrings {
    /// op.title
    public static let opTitle = "The operator wants to run a command"
    /// choices.heading
    public static let choicesHeading = "Your answer"
    /// choices.applied
    public static let choicesApplied = "Applied on the host at once, as the operator. The PM is told and does not ask you again."
    /// answer.action
    public static let answerAction = "Answer…"
    /// deny.action
    public static let denyAction = "Deny…"
    /// step.of
    public static let stepOf = "{n} of {total}"
    /// choices.none
    public static let choicesNone = "Choose an answer"
    /// choice.once.title
    public static let choiceOnceTitle = "Approve once"
    /// choice.once.detail
    public static let choiceOnceDetail = "Runs this command now. The next command asks again."
    /// choice.similar.title
    public static let choiceSimilarTitle = "Approve + allow similar for the session"
    /// choice.similar.detail
    public static let choiceSimilarDetail = "Runs it now, then lets this command or commands that start the same way run without asking."
    /// choice.auto.title
    public static let choiceAutoTitle = "Auto-approve all"
    /// choice.auto.detail
    public static let choiceAutoDetail = "Runs it now and opens a window that approves almost everything, for 1 hour or this session."
    /// once.confirm.action
    public static let onceConfirmAction = "Approve once"
    /// action.cancel
    public static let actionCancel = "Cancel"
    /// action.back
    public static let actionBack = "Back"
    /// scope.title
    public static let scopeTitle = "Allow similar commands"
    /// scope.exact.title
    public static let scopeExactTitle = "This exact command"
    /// scope.exact.detail
    public static let scopeExactDetail = "Only the command above, exactly as written."
    /// scope.prefix.title
    public static let scopePrefixTitle = "Commands that start with:"
    /// scope.prefix.wrapped
    public static let scopePrefixWrapped = "One value, shown whole"
    /// scope.rule
    public static let scopeRule = "A grant matches whole words. Pushes and deletes still ask."
    /// scope.length
    public static let scopeLength = "Lasts for this session, up to the host's grant limit."
    /// scope.action
    public static let scopeAction = "Approve and allow"
    /// auto.title
    public static let autoTitle = "Auto-approve all"
    /// auto.hour.title
    public static let autoHourTitle = "For 1 hour"
    /// auto.hour.end
    public static let autoHourEnd = "Lasts up to 1 hour (less when the host's limit is shorter)."
    /// auto.session.title
    public static let autoSessionTitle = "For this session"
    /// auto.session.end
    public static let autoSessionEnd = "Lasts until the host's configured limit."
    /// auto.end.both
    public static let autoEndBoth = "Both end when the controller restarts or when the window is revoked with cstan op revoke at the terminal."
    /// auto.coverage.heading
    public static let autoCoverageHeading = "What the window approves"
    /// auto.coverage
    public static let autoCoverage = "The window approves every Operator command and every worker permission prompt, pushes and deletes included, except the floor list: deletes or writes outside the project, and a controller restart. Scripts and shells are approved unchecked. Web fetch, MCP and other tool prompts are answered without a floor check."
    /// auto.next
    public static let autoNext = "Continue"
    /// auto.confirm.title
    public static let autoConfirmTitle = "Open an auto-approve window?"
    /// auto.confirm.action
    public static let autoConfirmAction = "Open window and approve"
    /// limited.lead
    public static let limitedLead = "Only Approve once and Deny are offered."
    /// limited.floor
    public static let limitedFloor = "This command matches the host's floor list, so it is approved one at a time."
    /// limited.floor.entry
    public static let limitedFloorEntry = "Floor entry"
    /// limited.restart
    public static let limitedRestart = "A controller restart is approved one at a time."
    /// limited.truncated
    public static let limitedTruncated = "The command is longer than the app can show (the first 4096 bytes are shown), so it can only be approved at the terminal."
    /// limited.not_checked
    public static let limitedNotChecked = "This proposal was made before the host could check it against the floor list, so it is approved one at a time."
    /// limited.text_mismatch
    public static let limitedTextMismatch = "The command does not match its hash, so it can only be denied."
    /// result.sending
    public static let resultSending = "Sending to the host…"
    /// result.once
    public static let resultOnce = "Approved once. The operator runs it now."
    /// result.grant
    public static let resultGrant = "Approved, and allowed for this session:"
    /// result.window
    public static let resultWindow = "Approved. Auto-approve window open"
    /// result.window.id
    public static let resultWindowId = "Window {windowId}"
    /// result.ends
    public static let resultEnds = "Ends {time}"
    /// result.window.revoke
    public static let resultWindowRevoke = "Revoke it at the terminal with cstan op revoke."
    /// result.denied
    public static let resultDenied = "Denied. The operator was told."
    /// result.pm
    public static let resultPm = "The PM was told and does not ask you again."
    /// refusal.nothing
    public static let refusalNothing = "Nothing was approved."
    /// refusal.busy
    public static let refusalBusy = "The operator is running a command. Try again when it finishes."
    /// refusal.not_waiting
    public static let refusalNotWaiting = "This proposal is no longer waiting. It was answered elsewhere or it expired."
    /// refusal.hash_mismatch
    public static let refusalHashMismatch = "The command changed since you opened it. Read it again before you answer."
    /// refusal.hash_mismatch.action
    public static let refusalHashMismatchAction = "Show the new command"
    /// refusal.needs_terminal
    public static let refusalNeedsTerminal = "The host no longer offers this choice for this command. Approve once, deny, or answer at the terminal."
    /// refusal.invalid_request
    public static let refusalInvalidRequest = "The host did not accept this answer."
    /// refusal.host_says
    public static let refusalHostSays = "Host message · untrusted text"
    /// oldhost.action
    public static let oldhostAction = "Send approval to PM"
    /// oldhost.note
    public static let oldhostNote = "Deny is applied on the host now. Approve goes to the project manager, who confirms with you and runs it."
}
