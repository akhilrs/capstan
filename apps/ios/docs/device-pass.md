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

Paste with the iPhone model, the iOS version and the commit (`git log --oneline -1`).
