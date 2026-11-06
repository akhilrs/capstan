# PM visibility pass (Mac, plan-152)

Run by hand on the Mac with a real host. Fill the **Observed** column (pass, fail and what you saw), paste the table
back, and each failure is fixed in its own commit named `fix(<package>): ...`. The iPhone half is in
[apps/ios/docs/device-pass.md](../../ios/docs/device-pass.md).

Before you start: the current `main` built from Xcode, one host added and live with a project that has a PM
registered, notifications allowed for Capstan (System Settings → Notifications), at least one notification kind on in
Settings. For the stuck checks set `pm_stuck_minutes = 2` in the host's config and restart the host. The stuck notification posts whenever the notification kinds list is not empty; there is no separate `pm_stuck` toggle.

Screenshots: take each of checks 1 to 9 in **light and dark**, and checks 1, 2 and 9 also in a **compact** window
(narrow enough for one column) and a **wide** one. Compare them with `docs/design/plan-152-design-pm/card-mac-*.png`
and `stuck-mac-*.png`; a difference is a finding.

| # | Area | Steps | Expected | Observed |
|---|---|---|---|---|
| 1 | Card, normal | Open the project's overview. | A "Project manager" block under the status strip: PM id (mono), activity word with glyph, "since" time, From the PM, You sent and Waiting on (two columns in a wide window). | |
| 2 | Card, compact window | Narrow the window until the columns no longer fit. | One column: messages above Waiting on; nothing is cut. | |
| 3 | Card, no PM | Open a project with no PM registered. | "No PM is active on <project>, so nobody routes work. The card fills in when a PM registers." | |
| 4 | Card, last known | Stop the host's network (or the host). | The card keeps its values with a grey "Last known · <time>" pill; Waiting on says "at <time>". | |
| 5 | Card, stale | With the host live, make the card's refresh fail once (block the read), wait for the next refresh. | Yellow "Stale · as of <time>" pill and a Refresh button; hovering Refresh shows "Refresh (⌘R)"; ⌘R and the button both refresh. | |
| 6 | Card, loading | Quit, start the app and open the project at once (or add a host and open its project before the snapshot arrives). | A skeleton shaped like the card in grey blocks with "Reading <host name>…"; with Reduce Motion on nothing moves. It is replaced by the card when the snapshot completes, and never shown for a stopped host. | |
| 7 | Card, nothing yet | A PM that has no message either way and nobody waits on. | "No message to you yet.", "Nothing yet. Write to <pm id> in Messages.", "Nobody waits on anything." | |
| 8 | You sent | In Messages, write to the PM while the host is offline, then go online. | The card shows your text at once from the local log with "Queued: the PM gets it when it is free"; then "Delivered to the PM · <time>"; then "Read by the PM · <time>". A failed one reads "Not delivered (expired)" in red with a triangle. | |
| 9 | Stuck banner, unread mail | Send the PM a message and keep the PM idle for 2 min. | A yellow banner "<pm id> looks stuck": "It is idle while your message has waited unread for 2 min, since <time>." with "PM messages" and "Pause project…". | |
| 10 | Stuck banner, no progress | Keep the PM shown as working without progress for 2 min. | "It shows as working but has made no progress for 2 min, since <time>." | |
| 11 | Stuck banner, prompt | Leave a prompt the PM relayed unanswered for 2 min. | "It has been stopped at a prompt for 2 min, since <time>. A prompt it relayed waits in Waiting for you." | |
| 12 | Notification, once | Let an episode open (check 9) with the app in the background. | One notification: title "PM looks stuck", the project name as subtitle, no body. Waiting longer posts no second one for the same episode. | |
| 13 | Notification tap | Click the notification. | Capstan comes to the front on that project's overview. | |
| 14 | Notification clears | Answer the PM so the episode ends. | The banner goes and the notification disappears from Notification Center. | |
| 15 | Alert false | End the episode, then within 30 min cause the same PM to get stuck again. | The banner shows; no notification is posted. | |
| 16 | Sidebar | While an episode is open, look at the project's row. | An 8 pt yellow ring before the name and "PM stuck" in the place of the count; no host text. When the project is not ready, its availability word shows instead. VoiceOver reads the name, "PM stuck" and the waiting count. After recovery the ring and word go and the count returns. | |
| 17 | Menu bar | While an episode is open, open the menu bar item. | Only the waiting count; nothing about the PM. | |
| 18 | ntfy push | With ntfy configured on the host, let an episode open. | A `pm_stuck` push arrives on the phone once for the episode. | |
