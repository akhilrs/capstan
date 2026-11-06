# Car pass (Siri and CarPlay)

The on-device pass for the voice shortcuts, run by hand on a **physical iPhone** in a car with CarPlay, or in the
CarPlay Simulator. Fill the **Result** column with **pass**, **fail** or **not available** and what you saw, then paste
the table back. A failure is reported, not worked around: the system decides what Siri may run in CarPlay.

Before you start: the current build installed, one host added and live, an Operator proposal waiting on it, and
Settings → Siri and CarPlay → **Decide by voice** on. Switch **Grants by voice** on for the rows that need it. Repeat
the rows marked (L) with the phone locked, unlocked, and right after a restart before the first unlock.

| # | Area | Steps | Expected | Result |
|---|------|-------|----------|--------|
| 1 | Widget | Tap the widget in the car or the simulator. | Capstan opens on what waits. | |
| 2 | Announcement, local | Let a local notification arrive for an Operator proposal (Time Sensitive on for its kind). | Siri reads the kind and the project, never the command. | |
| 3 | Announcement, ntfy | Same, from an ntfy notification. | The same. | |
| 4 | What's waiting (L) | Say "What's waiting in Capstan". | The count, then each item as kind, project and age. A host that did not answer is named as not read. | |
| 5 | Approve once (L) | Say "Approve once in Capstan". | Siri reads the project, the command inside "The command is: … End of command.", then asks "Approve it?". Yes sends; the result is spoken. | |
| 6 | Deny (L) | Say "Deny in Capstan", answer yes. | "Denied in <project>." | |
| 7 | Yes and no by voice | Answer the confirmation by saying yes, then no on another try. | Yes sends once; no says nothing was sent. | |
| 8 | Prefix by number | Grants on, command with a prefix grant offered. Say "Approve in Capstan", pick the prefix, then a number. | Siri names each prefix as a numbered choice, reads the chosen one back, asks, sends. | |
| 9 | Window and second yes | Grants on. Say "Approve in Capstan", pick the one-hour window. | The coverage is read out, then "Approve and open the window?", then a last check naming the project. A no at either stops it. | |
| 10 | Changed item refused | Let the proposal change on the host between question and answer. | "That command changed or is gone. Nothing was decided." | |
| 11 | Host off refused | Turn Decide by voice off, ask to approve. | "Decide by voice is off. …" Nothing is sent. Listing still works. | |
| 12 | Slow host | Make one host slow or unreachable. | It is named as not read; the other host's items are read. | |
| 13 | Background between question and answer | Start an approve, send the app to the background (or lock) before answering. | The answer still works or the run ends with nothing sent. | |
| 14 | Face ID on, cold start | Settings → Require Face ID on. Quit the app, then ask Siri. | No Face ID prompt; the flow runs. | |
| 15 | Background refresh | After a voice run, wait for a background refresh. | Badge and notifications still update. | |
| 16 | Cut run | Interrupt Siri during the read-back. | Nothing is sent. | |

Record the iOS version, the car or simulator, and anything Siri refused to run in CarPlay (the plan expects this may
happen for confirmations or numbered choices).
