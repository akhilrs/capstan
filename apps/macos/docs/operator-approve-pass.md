# Operator approve pass (Mac, plan-153)

Run by hand on the Mac with a real host. Fill the **Observed** column (pass, fail and what you saw), paste the table
back, and each failure is fixed in its own commit named `fix(<package>): ...`.

Before you start: the current `main` built from Xcode, one host added and live with a project whose Operator is
running (a host from this release, so its items carry `approveChoices`). Have commands ready that the Operator proposes:
a plain one (`git status`), one with offered prefixes (`git commit -m "x"`), one on the host's floor list, one longer than
4096 bytes, and one with a prefix longer than a line. For the old-host checks use a host from before `approve-as`.

Screenshots: take checks 1 to 12 in **light and dark**, and checks 3, 6 and 9 also at the **largest text size**
(System Settings, Accessibility, Display, Text size at its maximum). Compare them with
`docs/design/plan-153-design-approve/*-mac-*.png`; a difference is a finding.

| # | Area | Steps | Expected | Observed |
|---|---|---|---|---|
| 1 | Inspector, choices | Select an Operator proposal in Waiting for you. | Under the command: the line "Applied on the host at once, as the operator…", then Deny… and Answer…. No "Send approval to PM". | |
| 2 | Step 1 | Press Answer… (or ⌘↩). | A sheet "Your answer" with Approve once, Approve + allow similar for the session, Auto-approve all; nothing chosen, the primary reads "Choose an answer" and is disabled. | |
| 3 | Approve once | Choose Approve once; press Return. | No step count; the primary reads "Approve once"; the sheet closes and the inspector shows "Approved once. The operator runs it now." and "The PM was told…". | |
| 4 | Allow similar | Choose Approve + allow similar; Continue. | "1 of 2" then "2 of 2"; "This exact command" and one row for each offered prefix, each prefix in mono in its box; the primary "Approve and allow" is disabled until a row is chosen. | |
| 5 | Grant result | Choose a prefix; Approve and allow. | The inspector shows "Approved, and allowed for this session:", the prefix in mono, and "Ends HH:MM" in the host's time; with exact: "This exact command". | |
| 6 | Long prefix | Open the scope step for a command with a prefix longer than one line, at the largest text size. | The prefix wraps whole inside its box, not trimmed, cut or joined to the words around it; "One value, shown whole"; the sheet scrolls. VoiceOver reads "Prefix, one value" then the text. | |
| 7 | Auto window | Choose Auto-approve all; Continue; choose For 1 hour; Continue. | The coverage text and the end texts show; an alert "Open an auto-approve window?" with Cancel as the default. Cancel leaves the sheet; "Open window and approve" applies. | |
| 8 | Window result | Apply check 7. | "Approved. Auto-approve window open", "Window win-N", "Ends HH:MM", "Revoke it at the terminal with cstan op revoke." Auto-session gives no Ends line when the host sent no time. | |
| 9 | Keyboard | In the sheet press Tab, ↑, ↓, Return and Esc. | Tab visits the rows, Back or Cancel, then the primary; the window behind is skipped; Return presses the primary, Esc cancels or goes back; a disabled primary is not focusable. | |
| 10 | Limited | Open a proposal on the floor list, then a restart, then one made before the floor check. | "Only Approve once and Deny are offered." with the reason (floor entry as untrusted text); step 1 shows only Approve once. | |
| 11 | Deny only | Open the proposal longer than 4096 bytes, then one whose text does not match its hash. | Deny… only, no Answer…, with the matching reason line. | |
| 12 | Refusals | Answer a proposal that was answered elsewhere; change the command on the host before answering; answer while the Operator runs a command. | Not waiting: grey, the sheet closes. Hash mismatch: red, "Show the new command" closes the sheet. Busy: yellow, the sheet stays and Approve once may be tried again. Each says "Nothing was approved." and the code. | |
| 13 | Old host | Open a proposal on a host without `approve-as`. | The earlier inspector: Deny… and "Send approval to PM", unchanged. | |
| 14 | Deny | Press Deny… on any proposal. | The earlier deny sheet; "Denied" as before. | |
