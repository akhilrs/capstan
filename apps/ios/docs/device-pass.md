# Device pass (Phase 3)

The whole on-device test pass of plan section 8.3, run by hand on a **physical iPhone** with a real host. Fill the
**Observed** column (pass, fail and what you saw), paste the table back, and each failure is fixed in its own commit
named `fix(<package>): ...` (plan 7.5).

Before you start: the current `main` built from Xcode onto the iPhone (Team set on both targets), one host added and
live, Settings → Require Face ID on, Settings → Live Activity when items wait on, and notifications allowed.

## Hostile text fixture

Several rows need an item whose text is hostile. On the host, make an agent (or the operator) propose a command, or
send a message to the PM, whose text is the block below. It holds markdown, a link, control and bidi characters and,
with the padding line repeated, more than 4096 bytes.

```text
**bold** _italic_ [click me](https://evil.example) <script>alert(1)</script>
‮reversed‬ tab:	end bell: ^G
https://evil.example/phish?u=you
(repeat this line until the text is over 4096 bytes) xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

## Checklist

| # | Area | Steps | Expected | Observed |
|---|---|---|---|---|
| 1 | Face ID, cold start | Force-quit Capstan, open it. | Face ID is asked before anything shows; no host is reached until it passes. | |
| 2 | Face ID, 59 s away | Unlock, go to the Home Screen for 59 s (Device checks 7 helps), come back. | No prompt. | |
| 3 | Face ID, 61 s away | Same, 61 s. | Face ID is asked. | |
| 4 | Face ID off | Settings → Require Face ID off; cold start and 2 min away. | Never asked. Turn it back on. | |
| 5 | Dead link | Device checks 6: Start, then airplane mode. | "No heartbeat" after about 3 poll intervals; channel ends within about 60 s. | |
| 6 | Reconnect | Airplane mode off; then Wi-Fi off with cellular on (host reachable over cellular or VPN), and back. | Waiting goes reconnecting → live by itself each time. | |
| 7 | Host key change | On the host, replace its SSH host key (or point the host at another machine with the same name). Reconnect. | Blocked: "host key changed", no way to trust anyway; remove and add again recovers. | |
| 8 | Revoked key | Remove this iPhone's line from `~/.ssh/authorized_keys`, Try again. | "Refused this iPhone's key" with the public line to add back. Add it back; Try again goes live. | |
| 9 | New phone | Restore a backup to another iPhone (or delete the app and reinstall). | "This iPhone needs a new key"; no silent ed25519. | |
| 10 | Hostile text, details | Open the hostile item. | Plain mono text: no bold, no link, no tap target; control and bidi characters shown harmlessly; "cut at 4096 bytes" with the bytes left out; Approve not offered for cut text. | |
| 11 | Hostile text, surfaces | With the hostile item waiting: lock the phone; look at the notification, the Live Activity and the Dynamic Island. | Kind and project only. None of the item's text anywhere. | |
| 12 | Notification once | Make a new item wait while the app is open, then while it is in the background. | One notification each, title from the kind, body the project; tapping opens the item. Not repeated on the next refresh. | |
| 13 | Badge | Compare the app icon badge with the Waiting count. | Equal. Goes to zero when nothing waits. | |
| 14 | Live Activity | Make items wait, answer them all. | Starts with the first item; counts follow; ends when nothing waits. Lock Screen and island as on board 15. | |
| 15 | Background refresh | Lock the phone for several hours (or Xcode → Debug → Simulate Background Fetch while it is in the background). | Badge and Live Activity updated; any new item notified; no Face ID prompt during the refresh. | |
| 16 | Widgets | — | Not run: widgets need an App Group, which a free Personal Team cannot sign (they wait for a paid team). | n/a |

## PM card and stuck alert (plan-152)

Run on a device or simulator against a host with a PM. Set `pm_stuck_minutes = 2` in the project's config on the host so an episode opens within minutes. The Mac pass in [pm-visibility-pass.md](../../macos/docs/pm-visibility-pass.md) covers the host side; compare each screen with the boards `card-ios-*` and `stuck-ios-*` in `docs/design/plan-152-design-pm/`. A difference is a finding unless it is named in the report and accepted.

| # | Area | Steps | Expected | Observed |
|---|---|---|---|---|
| 17 | Card, normal | Hosts → project. | "Project manager" sits after the status card and before Waiting for you: PM id in mono, activity word and glyph, since; From the PM and You sent (two lines each, mono, delivery line under yours); Waiting on rows 44 pt tall with a chevron and the oldest age; a row shows a pressed fill. | |
| 18 | Card, links | Tap From the PM, You sent, then Worker reports, Reviews, Verify runs, Plan reviews, You. | The first two open the PM conversation; the four plan kinds open Plans; You opens Waiting for you. A row of an unknown kind has no chevron and does nothing. | |
| 19 | Card, no PM | Open a project with no active PM. | "No PM is active on <project>, so nobody routes work. The card fills in when a PM registers." | |
| 20 | Card, last known | Airplane mode with the project open. | Grey "Last known · <time>", "Working at <time>", "as of <time>" beside the heading, Waiting on says "at <time>". | |
| 21 | Card, stale | Host live, make the last refresh fail (stop the host's `cstan` read for one refresh). | Yellow "Stale · as of <time>" and the pull-to-refresh hint; pulling down refreshes. | |
| 22 | Card, loading and nothing yet | Add a host and open its project at once; then a project with no messages and nobody waiting. | Skeleton with "Reading <host>…"; then "No message to you yet.", "Nothing yet. Write to pm-1 in Messages.", "Nobody waits on anything." | |
| 23 | Stuck, each reason | With `pm_stuck_minutes = 2`: leave a message to the PM unread while it idles; let it show working without progress; leave it at a prompt. | One banner in the card per reason, with its line (unread mail, no progress, prompt); title "pm-1 looks stuck". | |
| 24 | Stuck, buttons | Tap PM messages; then Pause project… on the banner. | PM messages opens the conversation. Pause project… opens the pause sheet with Cancel focused and the reason "pm-1 looks stuck" filled in and editable; Cancel changes nothing. The "…" menu's Pause project… opens it with an empty reason. | |
| 25 | Stuck notification | Lock the phone while an episode opens. | One Lock Screen notification "PM looks stuck" with the project name, no body and no host text; not repeated while the episode lasts; tapping opens the PM conversation. | |
| 26 | Stuck, recovery | Let the PM recover (or answer its mail). | The banner goes; the notification is removed at the next refresh (pull to refresh, or a background refresh). | |
| 27 | Stuck, alert false | Start an episode the host reports with alert false (the PM card's stuck member has `alert` false; see the Mac pass for the host setting). | The banner shows; no notification is posted. | |
| 28 | Suspended app | Background the app, let an episode end. | The notification may stay until the next refresh: iOS runs background refresh when it chooses, and ntfy is the timely channel. Not a failure. | |
| 29 | Look | Items 17, 23 and 24 in light, dark and at AX3 (Settings → Accessibility → Display & Text Size → Larger Text). | Rows stack the value under the label with the chevron spanning both; banner buttons one per row; text never cut except the two-line previews. Attach the screenshots. | |

## Approve sheet for Operator proposals (plan-153)

Run against a host that sends `approveChoices` (the plan-153 host), with an Operator proposal waiting each time. Compare each screen with the boards `choices-ios-*`, `scope-ios-*`, `auto-ios-*`, `limited-ios-*`, `result-ios-*`, `refusal-ios-*` and `oldhost-ios-*` in `docs/design/plan-153-design-approve/`; the words are those of its `strings.json`. After each approval check on the host with `cstan op grants` and `cstan op windows` that exactly what the result lines say was made, and that the PM was told and does not ask again.

| # | Area | Steps | Expected | Observed |
|---|---|---|---|---|
| 30 | Approve once | Open the proposal, Answer…, pick Approve once, Approve once. | Applies from step 1 with no count; the sheet closes; "Approved once. The operator runs it now." and no grant or window line. The next command asks again. | |
| 31 | Approve exact | Answer…, Approve + allow similar, Continue, This exact command, Approve and allow. | "Approved, and allowed for this session:" with the exact command. The same command proposed again is approved by the grant; another command asks. | |
| 32 | Approve prefix | Answer…, Approve + allow similar, Continue, pick a "Commands that start with" row, Approve and allow. Then have the operator propose another command that begins with that prefix. | Result shows the prefix exactly as offered. The next matching proposal is approved by the grant without asking; a command that does not begin with the prefix, and a push or delete, still ask. | |
| 33 | Auto, 1 hour | Answer…, Auto-approve all, Continue, For 1 hour, Continue, Open window and approve. | The confirm step names the coverage and its default is Cancel; the result shows the window id and "Ends <host time>" (an hour from now, or less if the host's limit is shorter) and the revoke line. `cstan op revoke` closes it. | |
| 34 | Auto, session | Same, For this session. | Window id and its end per the host's limit; no window opens before the confirm (Cancel on the confirm leaves nothing open). | |
| 35 | Floor command | Have the operator propose a command on the host's floor list (a delete outside the project). | The sheet offers Approve once only, with the floor entry and "This command matches the host's floor list…". No similar and no auto row. | |
| 36 | Deny | Deny… on the detail, confirm. | The deny sheet is the existing one; "Denied. The operator was told."; nothing was approved. | |
| 37 | PM picker closing | With the sheet open on the detail, answer the same proposal from the PM's picker (or the terminal). Then tap the sheet's primary. | The sheet says the proposal is no longer waiting and offers only Back; nothing was approved twice. | |
| 38 | Changed command | Open the sheet, change the proposal on the host (new text, same id), tap the primary. | "Nothing was approved." with the changed-command text; Show the new command closes the sheet and reads it again. | |
| 39 | Old host | Use a host older than `approve-as` (no `approveChoices`). | No sheet. The bar offers Deny… and "Send approval to PM"; the note under the command is the old-host note; Send approval to PM works as before. | |
| 40 | Largest type | Settings → Accessibility → Larger Text at the largest size. Open the sheet on a proposal with a long prefix (60 or more characters, with spaces) and go to the scope step. | The prefix is shown whole and wrapped on several lines, not cut or ellipsized, in mono, character for character as offered; buttons stack and stay reachable; every step scrolls. | |

Paste with the iPhone model, the iOS version and the commit (`git log --oneline -1`).
