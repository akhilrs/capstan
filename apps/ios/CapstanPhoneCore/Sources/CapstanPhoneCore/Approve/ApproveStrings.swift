import Foundation

/// The words of the approve sheet, one constant per key of docs/design/plan-153-design-approve/strings.json, verbatim.
/// `scripts/check-approve-strings.py` fails when a key or a value here differs from that file. Each line ends with
/// `// key: <key>` so the check can read it; a value uses only the escapes \\ and \".
public enum ApproveStrings {
    public static let opTitle = "The operator wants to run a command" // key: op.title
    public static let choicesHeading = "Your answer" // key: choices.heading
    public static let choicesApplied = "Applied on the host at once, as the operator. The PM is told and does not ask you again." // key: choices.applied
    public static let answerAction = "Answer…" // key: answer.action
    public static let denyAction = "Deny…" // key: deny.action
    public static let stepOf = "{n} of {total}" // key: step.of
    public static let choicesNone = "Choose an answer" // key: choices.none
    public static let choiceOnceTitle = "Approve once" // key: choice.once.title
    public static let choiceOnceDetail = "Runs this command now. The next command asks again." // key: choice.once.detail
    public static let choiceSimilarTitle = "Approve + allow similar for the session" // key: choice.similar.title
    public static let choiceSimilarDetail = "Runs it now, then lets this command or commands that start the same way run without asking." // key: choice.similar.detail
    public static let choiceAutoTitle = "Auto-approve all" // key: choice.auto.title
    public static let choiceAutoDetail = "Runs it now and opens a window that approves almost everything, for 1 hour or this session." // key: choice.auto.detail
    public static let onceConfirmAction = "Approve once" // key: once.confirm.action
    public static let actionCancel = "Cancel" // key: action.cancel
    public static let actionBack = "Back" // key: action.back
    public static let scopeTitle = "Allow similar commands" // key: scope.title
    public static let scopeExactTitle = "This exact command" // key: scope.exact.title
    public static let scopeExactDetail = "Only the command above, exactly as written." // key: scope.exact.detail
    public static let scopePrefixTitle = "Commands that start with:" // key: scope.prefix.title
    public static let scopePrefixWrapped = "One value, shown whole" // key: scope.prefix.wrapped
    public static let scopeRule = "A grant matches whole words. Pushes and deletes still ask." // key: scope.rule
    public static let scopeLength = "Lasts for this session, up to the host's grant limit." // key: scope.length
    public static let scopeAction = "Approve and allow" // key: scope.action
    public static let autoTitle = "Auto-approve all" // key: auto.title
    public static let autoHourTitle = "For 1 hour" // key: auto.hour.title
    public static let autoHourEnd = "Lasts up to 1 hour (less when the host's limit is shorter)." // key: auto.hour.end
    public static let autoSessionTitle = "For this session" // key: auto.session.title
    public static let autoSessionEnd = "Lasts until the host's configured limit." // key: auto.session.end
    public static let autoEndBoth = "Both end when the controller restarts or when the window is revoked with cstan op revoke at the terminal." // key: auto.end.both
    public static let autoCoverageHeading = "What the window approves" // key: auto.coverage.heading
    public static let autoCoverage = "The window approves every Operator command and every worker permission prompt, pushes and deletes included, except the floor list: deletes or writes outside the project, and a controller restart. Scripts and shells are approved unchecked. Web fetch, MCP and other tool prompts are answered without a floor check." // key: auto.coverage
    public static let autoNext = "Continue" // key: auto.next
    public static let autoConfirmTitle = "Open an auto-approve window?" // key: auto.confirm.title
    public static let autoConfirmAction = "Open window and approve" // key: auto.confirm.action
    public static let limitedLead = "Only Approve once and Deny are offered." // key: limited.lead
    public static let limitedFloor = "This command matches the host's floor list, so it is approved one at a time." // key: limited.floor
    public static let limitedFloorEntry = "Floor entry" // key: limited.floor.entry
    public static let limitedRestart = "A controller restart is approved one at a time." // key: limited.restart
    public static let limitedTruncated = "The command is longer than the app can show (the first 4096 bytes are shown), so it can only be approved at the terminal." // key: limited.truncated
    public static let limitedNotChecked = "This proposal was made before the host could check it against the floor list, so it is approved one at a time." // key: limited.not_checked
    public static let limitedTextMismatch = "The command does not match its hash, so it can only be denied." // key: limited.text_mismatch
    public static let resultSending = "Sending to the host…" // key: result.sending
    public static let resultOnce = "Approved once. The operator runs it now." // key: result.once
    public static let resultGrant = "Approved, and allowed for this session:" // key: result.grant
    public static let resultWindow = "Approved. Auto-approve window open" // key: result.window
    public static let resultWindowId = "Window {windowId}" // key: result.window.id
    public static let resultEnds = "Ends {time}" // key: result.ends
    public static let resultWindowRevoke = "Revoke it at the terminal with cstan op revoke." // key: result.window.revoke
    public static let resultDenied = "Denied. The operator was told." // key: result.denied
    public static let resultPm = "The PM was told and does not ask you again." // key: result.pm
    public static let refusalNothing = "Nothing was approved." // key: refusal.nothing
    public static let refusalBusy = "The operator is running a command. Try again when it finishes." // key: refusal.busy
    public static let refusalNotWaiting = "This proposal is no longer waiting. It was answered elsewhere or it expired." // key: refusal.not_waiting
    public static let refusalHashMismatch = "The command changed since you opened it. Read it again before you answer." // key: refusal.hash_mismatch
    public static let refusalHashMismatchAction = "Show the new command" // key: refusal.hash_mismatch.action
    public static let refusalNeedsTerminal = "The host no longer offers this choice for this command. Approve once, deny, or answer at the terminal." // key: refusal.needs_terminal
    public static let refusalInvalidRequest = "The host did not accept this answer." // key: refusal.invalid_request
    public static let refusalHostSays = "Host message · untrusted text" // key: refusal.host_says
    public static let oldhostAction = "Send approval to PM" // key: oldhost.action
    public static let oldhostNote = "Deny is applied on the host now. Approve goes to the project manager, who confirms with you and runs it." // key: oldhost.note
}
