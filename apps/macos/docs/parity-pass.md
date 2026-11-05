# Mac parity pass

The by-hand test pass for plan-149 (Mac app parity with the recent iPhone app changes), run on a **Mac** with a real host. Run the commands from the repository root. For each check, fill the **Observed** column (pass, or fail and what you saw), and paste the table back. For a failure, send what the **On failure** column asks for.

Before you start: the integration branch of plan-149 checked out on the Mac, Xcode 26 selected (README, Build and run, step 0), the app signed with your Team, and one host added and live (README, Checking a host). The host needs the PM-410 change so that a PM message is kept whole up to 4096 bytes; without it the merge refusal fix (check 8) is cut short. Use a project with a PM running, one integration that passed review and verify, one draft plan, and one approved plan.

Known gaps against the design. They are listed, not failures; do not report them: no loading-first-snapshot state, no open/close-all for the groups, no keyboard arrows in the lists.

## A. Build and tests

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 1 | Package tests | `swift test --package-path apps/macos/CapstanKit`, then `swift test --package-path apps/macos/CapstanAppCore`, then `swift test --package-path apps/ios/CapstanPhoneCore` | Each ends with all tests passed. | The lines with `error:` or `failed`, and the Xcode version. | |
| 2 | Mac build | `xcodebuild -project apps/macos/Capstan.xcodeproj -scheme Capstan -configuration Debug -destination 'platform=macOS' build` | `BUILD SUCCEEDED`. | The lines that contain `error:`, and the Xcode version. | |
| 3 | iPhone build | `xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'generic/platform=iOS Simulator' build` | `BUILD SUCCEEDED`. | The lines that contain `error:`, and the Xcode version. | |
| 4 | Static checks | `apps/macos/scripts/check-static.sh` and `python3 apps/macos/scripts/check-xcodeproj.py` | Both exit 0. | The output. | |
| 5 | Run | Run the Capstan scheme from Xcode, or open the built app. | The window opens with the host live. | A screenshot of the status capsule and the Hosts screen. | |

## B. Merge and discard

An integration that waits to be merged shows **Merge…** in its inspector. Before the merge checks, note `git -C <project> rev-parse HEAD` and `git -C <project> status --short` on the host.

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 6 | Merge into the checked-out branch | Open the integration, choose Merge. Leave **Into** on the checked-out branch, press Merge. | Result "Merged, not pushed" with the branch, the first 12 characters of the merge commit and the time; the PM is told. Nothing is pushed: the sheet has no push result. The checkout holds the merge commit. | A screenshot of the sheet, the host's `git log --oneline -3`, and the Messages thread. | |
| 7 | Merge into another branch | Make a second branch on the host that is not checked out. Merge another integration with **Into** set to it. | Merged into that branch. On the host, `git status --short` and `HEAD` of the checkout are as before; the checkout is untouched. | The same, plus the Into list as shown. | |
| 8 | Refusal and Ask the PM | Edit a tracked file in the checkout (dirty tree) and press Merge. | "Refused: the working tree has changes", "Nothing was merged", the files named in a "From the host · untrusted text" block, and a button **Ask the PM to fix this**. Press it: the sheet says "Sent to the PM", and the message appears in Messages. | A screenshot of the refusal and the host's text. | |
| 9 | Other refusals | Merge while the checkout is on a different branch than Into (wrong branch); an integration whose review failed (not reviewed); an integration that is running (busy). | Wrong branch and not reviewed: refused with **Ask the PM to fix this**. Busy: "an integration is running", **Done** only. | A screenshot and the refusal word. | |
| 10 | Paused project | Run `cstan pause --reason "parity pass"` on the host, then Merge. | "Refused: the project is paused" with **Resume project**. Press it; then **Merge again** merges. | A screenshot of each step. | |
| 11 | Conflict | Make the branch conflict with the checkout's branch; Merge. | "The merge has conflicts", "Nothing changed in the checkout", **Ask the PM to fix this**. The checkout is clean. | A screenshot and `git status --short` on the host. | |
| 12 | Unreachable host | Turn the host off (or Wi-Fi off) and Merge. | "Could not reach <host>" or "No answer yet", with **Retry**; Retry asks again and never merges twice. | A screenshot. | |
| 13 | Discard | On an integration choose **Discard integration…**. | A sheet "Discard this integration?" with the branch. Escape keeps it. Confirm with **Discard**: banner "Discarded. The host removed the branch." | A screenshot and `git branch` on the host. | |
| 14 | Design approval | Make an item of kind design wait with a link. Open it. | The link is shown; **Open in browser…** asks "Open this link?" and only opens http and https; the approve and changes buttons work and the item leaves the Waiting list. Do the same on the iPhone with another design item. | A screenshot of the inspector. | |

## C. Cancel a plan and a draft

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 15 | Cancel plan | Plans, choose the approved plan, More actions, **Cancel plan…**. | A sheet "Cancel this plan?" with the plan, its state and its title; **Keep plan** is the default (escape or return keeps it). **Cancel plan** shows "Cancelling…" and then "Plan cancelled". The packages stop. | A screenshot and `cstan plan show <plan>` on the host. | |
| 16 | Cancel draft | The same on the draft plan. | The sheet reads "Cancel this draft?"; the draft is withdrawn and the PM is told. | The same. | |

## D. Messages to the PM

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 17 | Compose popover | Press shift-command-N (or **Message the PM**). | A popover with **To** (the project's PM). With no PM running, it says "No project has an active PM, so nothing can be sent." | A screenshot. | |
| 18 | Return and Shift-Return | In the Messages composer type a line, press shift-Return, type another, press Return. | Shift-Return adds a line; Return sends both lines as one message. The hint "Return sends · shift-Return adds a line" is shown. | What was sent. | |
| 19 | Sent, then Delivered | Send a message and watch the thread. | Sending, then Sent (on the host), then Delivered once the PM has it. | The states seen, with times. | |
| 20 | Retry | Send with the host off. | "Not sent · <reason>" with **Retry**. Turn the host on, press Retry: the message goes once. | A screenshot, and the host's `cstan inbox` count of the message. | |
| 21 | Size limit | Paste text near and over the limit (4000 bytes, in UTF-8). | A counter "N of 4000 bytes"; over the limit the field turns red and Send is off; at 4000 bytes or fewer Send works. A message with multi-byte characters counts bytes, not characters. | A screenshot of the counter. | |
| 22 | Whole message | Send a message of about 3900 bytes. | The PM receives it whole (needs PM-410 on the host). | Where the text ended. | |
| 23 | Question with choices | Make the PM ask a question with options. | The question shows as "Asks you" with choice buttons in the inspector and the thread. Press one: the question reads "Answered" and the PM has the answer. Answering by text works too ("Answering question N from <agent>"). | A screenshot. | |
| 24 | Read kept across relaunch | Receive a PM message and read it. Quit the app (command-Q), start it again. Then receive another and quit without reading. | The read message stays read; the unread one is still unread, with its badge. | The badge and thread states before and after. | |
| 25 | New message at once | With the app open, make the PM send a message. | It appears in the thread and the Messages badge goes up at once, without a refresh. | The time it took. | |
| 26 | Unread count | Compare the Messages sidebar badge with the unread threads. | Equal; zero when all are read. | A screenshot. | |

## E. Agents, Plans and the sidebar

Use at least 6 projects on 2 hosts, one project with more than 4 agents and one with more than 10 plans.

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 27 | Agents, all projects | Open Agents. | Groups "Needs attention · N", "Pinned · N" and "Other projects · N". A project opens in place and shows 4 agents, then "Show N more". The filter shows counts. | A screenshot. | |
| 28 | Agents filter and search | Pick a filter; search for an agent. | Filtered or empty states carry "Show all agents" or "Clear search". Search and a filter show matches in full, with no "Show N more". | A screenshot. | |
| 29 | Pins | Context menu on a project: **Pin** / **Unpin**, and **Show only <project>**. Quit and start again. | The pin moves the project to "Pinned" and stays after relaunch. Show only limits the list to that project. | A screenshot. | |
| 30 | Plans | Open Plans; use the project menu. | "Needs you · N" first, then Active, In review, Done. A group shows 10 plans, then "Show N more". Search matches titles and ids ("No results for ..." with Clear search). | A screenshot. | |
| 31 | Sidebar badges | Make an agent need attention and a plan need you. | An attention badge on Agents and a needs-you badge on Plans; they clear when handled. | A screenshot. | |
| 32 | Host groups | Press the chevron beside a host in the sidebar. | The host's projects fold ("Hide projects" / "Show projects") and open again; the state holds while the app runs. | A screenshot. | |

## F. Notifications and the menu bar

| # | Check | Steps | Expected | On failure | Observed |
|---|---|---|---|---|---|
| 33 | No host text | Make an item wait whose text is hostile (markdown, a link, `<script>`, bidi characters). Look at the notification, the menu bar list and the Notification Center. | Kind and project only. None of the item's text. Opens the item on click. | A screenshot of the notification. | |
| 34 | Notification once | Make an item wait with the app open, then in the background. | One notification each; not repeated on the next refresh. | What was shown. | |
| 35 | Menu bar count | Make 1, then 7 items wait; answer them. | The menu bar item shows the count; its list shows the newest and "N more"; "Nothing is waiting." when empty; **Open Capstan** and **Quit Capstan** work. | A screenshot at 7. | |

Paste the table with the Mac model, the macOS and Xcode versions, and the commit (`git log --oneline -1`). Each failure is fixed in its own commit.
