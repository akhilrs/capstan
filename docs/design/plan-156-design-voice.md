# Spoken text for car announcements and Siri approvals (plan-156, design-voice)

Status: **for the user's approval** (`cstan design approve`), **revision 3**. The user picked **direction A, Short lines** (2026-11-05, through pm-1), then asked that Siri offer everything the phone offers for an Operator proposal: approve once, this exact command for the session, a prefix for the session, auto-approve for 1 hour or the session, and deny. plan-156 replaces plan-155. This revision moves the plan-155 work (commit 8015012) to the plan-156 names and adds architect-61's points:
- untrusted text is read only when verified and whole, inside a fixed frame;
- a command with any character outside printable ASCII is read only;
- a spoken refusal before the first unlock;
- Time Sensitive starts off for every kind;
- `check.py` gains the `phrase.*` / `prompt.*` contract and the two caps.

Design only; no implementation code. car-notify and the voice packages start after approval.

- **Design (Claude Design, 4 boards, version 7):** https://claude.ai/artifact/7i9KMEc6aTCHCR7uFtkdww
- Direction A, Short lines (picked): the same canvas, board "Overview". Direction B, Guided (not picked): https://claude.ai/artifact/9v3Bd5xSQ9MVY2HeRLoAJC
- `strings.json`: every text the app packages copy verbatim, 130 keys, placeholders in braces. `rules.json`: the rules below in a form tests can read. `check.py`: the contract check (see Strings).
- PNGs:
  - `settings-light.png` and `settings-dark.png`: the build spec for the three settings rows.
  - `announce-light.png`, `announce-dark.png`, `dialogs.png` and `overview.png`.
  - The directions are in `directions/`.
- Board sources are in `source/`. `contrast.js` is the computed contrast check.

The settings boards follow `docs/design/iphone-app.md` (direction A, Glass Inbox): inset grouped lists, iOS text styles and the record's colour roles. The approve choices and their texts follow plan-153's approve sheet (`docs/design/plan-153-design-approve.md`), turned into short spoken lines. Hosts, projects and commands are example data.

## The two caps

- **LINE_CAP = 120** characters: the fixed words of one spoken line. They are measured with long example values in `{project}`, `{host}` and `{where}`, and with the item text left empty.
- **ITEM_CAP = 120** characters: the item text alone, counted separately from the fixed words. The item text is the command, a prefix or a project name.
- A command over ITEM_CAP is listed as "too long" and cannot be decided by voice, not even denied. Every truncated command is over it.
- The Swift constants are copied from these two numbers. `check.py` holds the same constants and fails if this note states different ones.

## Brief

**Users and task.** The person who runs Capstan, driving with CarPlay. Eyes on the road, hands on the wheel, the phone out of reach and often locked. What must be fast and safe: hear that something waits; hear the exact command; answer it by voice with the same choices the phone offers, without picking up the phone.

**Purpose.** Words, not screens.
- Siri reads Time Sensitive notifications aloud: the kind and the project only.
- Four App Shortcuts read what waits, approve once, approve with a choice and deny Operator proposals, always after a framed word-for-word read-back.
- Settings rows switch this on.

**Tone.** Short, exact, calm. **Axes (1 to 5):** task intensity 4 (operate), data density 2 (one item at a time, by ear), brand expression 1, audience 2 (expert).

**References (traits only).**
- Apple Maps spoken guidance: one instruction per sentence, the noun first, no filler. Used for line length and order.
- Siri reading Messages in CarPlay: "X says …", then the body, then a yes/no question. Used for read-back then confirm.
- Aviation readback: the pilot repeats the clearance word for word before acting on it. Used for the read-back rule.
- Phone menus that read numbered options: a short numbered list that a caller answers by number. Used for the choice and the prefixes.
- Apple Wallet Express Transit: a convenience that skips Face ID, off until the person turns it on, with its scope in the footer. Used for the two voice switches.

**Constraints.**
- Platform: iOS 26, SwiftUI, App Intents (`openAppWhenRun` false, `authenticationPolicy .alwaysAllowed`, `requestConfirmation`, parameter disambiguation), UserNotifications with `interruptionLevel .timeSensitive`.
- No push (D8) and no CarPlay entitlement.
- Keys are stored AfterFirstUnlock.
- Siri may cut or reword long dialogs in CarPlay (plan risk), so each line is one sentence within LINE_CAP.
- No agent or host text in a notification.
- WCAG 2.2 AA on the settings rows; VoiceOver reads the same strings.
- Never ask the driver to pick up the phone.

**What was read first.**
1. DESIGN.md: none. The `cstan design guide rules` rules apply. There is no `docs/design/directions.md` or `rules.md`.
2. Design system: Capstan (Claude Design), installed on the canvas. The iPhone record overrides it on app surfaces.
3. Brand: rope amber `#8a521c` (light) / `#e0a45e` (dark).
4. Current UI:
   - `SettingsScreen.swift`;
   - `NotificationsSettingsScreen.swift`, with the Time Sensitive section, which has an empty default today;
   - `PhoneNotificationPlan.swift`;
   - plan-153's approve sheet strings.
5. Platform: above.
6. Content extremes:
   - commands up to 4096 bytes, multi-line or non-ASCII;
   - up to 3 prefixes;
   - project names up to 64 characters;
   - counts in the hundreds;
   - ages from seconds to days.
7. Users: above.
8. WCAG 2.2 AA.
9. Scope: design only, 4 hours.

Research: `docs/research/carplay-siri-capstan.md`.

## Directions offered

| | A Short lines (picked) | B Guided |
|---|---|---|
| Catalog | Control Room + Plain Speech | Plain Speech + Conversation |
| Count | "3 items wait in Capstan." | "… 1 command, 1 design and 1 permission prompt. I'll read them, oldest first." |
| Read-back | "Approve once in payments-api. The command is: git push origin main. End of command." | "The oldest command waits in payments-api. It reads: … Approving runs it this one time and allows nothing else." |
| Confirm | "Approve it?" | "Do you want to approve it once?" |
| Density / audience | fewer words, expert | more words, broader |

The two differ on data density and audience. A was recommended and picked because Siri may shorten long dialogs in CarPlay.

## Boards and states

| Board | Shows | States |
|---|---|---|
| Overview | The whole flow in one script, beside the settings rows. | default |
| Settings rows (light, dark) | Settings with the **Siri and CarPlay** section: **Decide by voice** and **Grants by voice**. Notifications with Time Sensitive per kind, every kind off, and the car footer. | both off (the default, grants disabled); Decide by voice on with keyboard focus; both on with focus on grants; a pressed row |
| Announcements (light, dark) | The six notifications, one per kind, with what Siri says. Drawn with Time Sensitive turned on for commands and prompts. | Time Sensitive on and off |
| Siri dialogs | Every spoken line with its key: the listing; approve once; approve with a choice; deny; not decidable; every result and refusal, including the locked phone and the unpaired phone. | empty, loading (Siri's own indicator, 8 s budget), error, success |

Hover does not exist on iPhone; the pressed state replaces it. Loading has no spoken line: Siri shows its working indicator while the hosts are read, and the 8 s budget ends in `refusal.timeout`.

## Rules for the user to confirm

Each rule gives the choice, the reason and what Siri says.

### Untrusted text

The command, a prefix and the project name come from the host or an agent.

1. **Read only verified, whole, printable text.**
   - *Choice:*
     - A command is spoken only when it is verified (UntrustedText), whole (not cut), at most ITEM_CAP characters, and every character is printable ASCII (U+0020 to U+007E).
     - Any other character makes it read only: a control character, a newline, a tab, a bidi mark, a non-ASCII letter or a look-alike letter.
     - The same rule and cap apply to the confirmation dialog shown on screen.
   - *Reason:* text Siri cannot say, or says differently from what the host runs, must not be approved by ear.
   - *Siri (list):* "A command in infra, 2 days ago. It can't be read aloud." / "… Too long to read aloud." / "… Its text could not be checked."
   - *Siri (decide):* "The command in infra can't be decided by voice: it has characters that can't be read aloud."
2. **The frame.**
   - *Choice:*
     - The command is always read as "The command is: {command}. End of command." (`frame.command`).
     - The question that follows is fixed app text (`confirm.*`), placed after the frame.
     - Prefixes get the same treatment: "Choice 2 is: git push origin. End of choice 2."
   - *Reason:* words inside a command, such as "say yes", are then heard as command text and never as Siri's question.
   - *Siri:* "Approve once in payments-api. The command is: git push origin main. End of command. Approve it?"
3. **Project names.**
   - *Choice:* a project name that is not verified, not printable ASCII or longer than ITEM_CAP is spoken as "a project whose name can't be read aloud" (`where.unreadable`). Its commands are not decidable by voice.
   - *Reason:* the read-back must name the project the decision goes to.
   - *Siri:* "The command in a project whose name can't be read aloud can't be decided by voice: its project name can't be read aloud."

### Which item and which choices

4. **Approve once.**
   - *Choice:*
     - Applies to an `op` item whose `approveChoices.once` is true and whose `limit` is null.
     - It is sent as `op approve-as once`.
     - On an old host (no `approveChoices`), Siri sends the forwarded `op approve`, which the PM reads as approve once.
     - Needs Decide by voice.
   - *Reason:* one command, one time: the narrowest yes.
   - *Siri:* "Approved once in payments-api." On an old host: "Sent to the PM of payments-api as approve once."
5. **Grants and auto windows by voice.**
   - *Choice:* with **Grants by voice** on, "Approve in Capstan" offers only the choices `approveChoices` offers, numbered in this order:
     1. once;
     2. this exact command, for the session (`approve-as exact`);
     3. commands that start the same way, for the session (`approve-as prefix --prefix <words>`);
     4. auto-approve for 1 hour (`approve-as auto-hour`);
     5. auto-approve for this session (`approve-as auto-session`).
   - Never full auto, and never on an old host.
   - *Reason:* the user asked for parity with the phone. The second switch keeps the wide answers off until the user chooses them.
   - *Siri:* "How do you want to approve it? Say the number." "1: once." "2: this exact command, for the session." …
6. **Prefix.**
   - *Choice:*
     - Siri reads each offered prefix inside its frame, numbered, shortest first, at most 3: "Choice 1 is: git push. End of choice 1."
     - You say the number.
     - Siri reads the chosen prefix back, then "A grant matches whole words. Pushes and deletes still ask. It lasts for this session, up to the host's grant limit."
     - Siri asks "Approve and allow?"
     - The prefix is sent byte for byte as the host offered it.
   - *Reason:* a prefix is wider than one command, so it is heard twice.
   - *Siri:* "And allow commands that start with choice 2: git push origin. End of choice 2."
7. **Auto windows: the coverage, then two yeses.**
   - *Choice:*
     - Siri reads the length.
     - Then the coverage: "The window approves every Operator command and worker permission prompt, pushes and deletes included." "Only the floor list still asks: deletes or writes outside the project, and a controller restart." "Scripts and shells are approved unchecked." "Web fetch, MCP and other tool prompts are answered without a floor check."
     - Then "Approve and open the window?" and, after a yes, the second yes (`prompt.secondYes.*`).
   - *Reason:* a window approves everything, so the driver hears what "everything" means.
   - *Siri:* "Last check: auto-approve everything in payments-api for 1 hour?" … "Window win-7. End it at the terminal with cstan op revoke."
8. **Deny only (limits).**
   - *Choice:*
     - These `op` items can be denied but not approved by voice:
       - an item whose `limit` is `floor`, `restart`, `text_mismatch` or `not_checked`;
       - a command with a shell operator `|`, `&&`, `;` or `>`.
     - `truncated` is never reached, because a cut command is not decidable at all (rule 1).
   - *Reason:* a hands-free yes is not the place to overrule the floor or a restart, and a chained command is hard to judge by ear. Deny is always safe.
   - *Siri:*
     - The line: "The command in infra can only be denied by voice: {reason}."
     - The reason for each case:

       | Limit or case | Reason Siri gives |
       |---|---|
       | floor | the approval floor holds it back |
       | restart | it restarts the controller |
       | not_checked | the approval floor did not check it |
       | text_mismatch | its text no longer matches |
       | shell operator | it chains, pipes or redirects commands |
9. **Read only (every other kind).**
   - *Choice:* prompts, designs, messages, merges and other items are read as kind, project and age only. The design name is not read.
   - *Reason:* only Operator proposals have a host-side decision that voice can send safely.
   - *Siri:* "A design in site, 1 hour ago." / "2 items wait, but only commands can be decided by voice."
10. **Target.**
    - *Choice:*
      - Approve once, approve and deny act on the oldest eligible Operator proposal across hosts.
      - The read-back names its project, and adds "on {host}" when another host has a project of the same name.
      - Older items that are skipped are counted.
    - *Reason:* the driver cannot pick from a list, so the rule is fixed and spoken.
    - *Siri:* "1 older command can't be approved by voice." "Approve once in payments-api on build-host. The command is: …"
11. **Read-back and confirmation.**
    - *Choice:*
      - The decision sends the hash of exactly what was read.
      - A "no" sends nothing.
      - If no number is heard, nothing is sent.
    - *Siri:* "That command changed or is gone. Nothing was decided." / "Nothing was sent." / "I didn't catch a number. Nothing was sent."

### Settings and state

12. **Decide by voice: off by default** (a missing key means off).
    - *Reason:* voice skips Face ID.
    - *Siri when off:* "Decide by voice is off. Turn it on in Capstan settings when you are parked." Listing still works.
13. **Grants by voice: off by default.** It is usable only with Decide by voice on; the row is disabled otherwise.
    - *Siri when off:* "Grants by voice is off, so only approve once is offered." Siri then continues as approve once.
14. **Locked phone.**
    - *Choice:* before the first unlock since the phone started, the keys cannot be read (AfterFirstUnlock). Every voice intent then speaks one fixed refusal and sends nothing. After the first unlock, voice works on a locked phone, as the footer says.
    - *Siri:* "This iPhone hasn't been unlocked since it started, so Capstan can't read its keys. Unlock it once. Nothing was sent."
15. **Time Sensitive per kind: empty by default** (unchanged from today).
    - *Choice:* every kind starts off. The user turns kinds on; car.md (car-notify) says which ones to turn on for the car.
    - *Reason:* Time Sensitive passes a Focus, so it should be the user's choice.
16. **Listing.**
    - *Choice:*
      - Oldest first across hosts.
      - At most 3 items are read, then "And {n} more."
      - The hint is spoken only when Decide by voice is on and a decidable command waits.
    - *Siri:* "To decide the command, say: approve in Capstan, or deny in Capstan."
17. **Not sent is not retried.** The client label ends with " voice".
    - *Siri:* "Could not reach build-host. Nothing was sent, and Capstan won't retry." / "Sent, but build-host did not answer. Check Capstan when you are parked."

### Every refusal and result

| Case | Key | Siri says |
|---|---|---|
| not unlocked since start | `refusal.locked` | This iPhone hasn't been unlocked since it started, so Capstan can't read its keys. Unlock it once. Nothing was sent. |
| not paired | `refusal.notPaired` | This iPhone isn't paired with a Capstan host. Pair it when you are parked. |
| hash_mismatch, not_waiting | `refusal.changed` | That command changed or is gone. Nothing was decided. |
| host unreachable | `refusal.unreachable` | Could not reach {host}. Nothing was sent, and Capstan won't retry. |
| read over budget | `refusal.timeout` | {host} took too long. Nothing was sent, and Capstan won't retry. |
| sent, no or malformed answer | `result.unknown` | Sent, but {host} did not answer. Check Capstan when you are parked. |
| voice decisions off | `refusal.voiceOff` | Decide by voice is off. Turn it on in Capstan settings when you are parked. |
| grants off | `choose.grantsOff` | Grants by voice is off, so only approve once is offered. |
| only once offered | `choose.onlyOnce` | Only approve once is offered for this command. |
| old host, approve with a choice | `choose.oldHost` | {host} offers only approve once. Update Capstan there for more. |
| no number heard | `choose.unclear` | I didn't catch a number. Nothing was sent. |
| deny only | `refusal.denyOnly` | The command in {project} can only be denied by voice: {reason}. |
| too long | `refusal.tooLong` | The command in {project} is too long to read aloud, so it can't be decided by voice. |
| not printable ASCII, multi-line, unverified, unreadable project | `refusal.notDecidable` + `reason.nonAscii` / `.multiline` / `.unverified` / `.project` | The command in {project} can't be decided by voice: {reason}. |
| nothing to decide | `refusal.nothing` | No command waits for you. |
| only read-only items | `refusal.readOnly.one` / `.many` | {n} items wait, but only commands can be decided by voice. |
| busy | `refusal.busy` | {host} is busy with another command. Nothing was decided. Try again soon. |
| needs_terminal | `refusal.needsTerminal` | The host now holds this back, so it needs the terminal. Nothing was decided. |
| limit_reached | `refusal.limitReached` | Too many requests for this command. Nothing was sent. |
| no_active_pm (forward) | `refusal.noActivePm` | {project} has no active PM, so nothing was sent. |
| invalid_request, any other code | `refusal.other` | {host} refused it. Nothing was decided. |
| applied, once | `result.approved` | Approved once in {project}. |
| applied, exact | `result.exact` (`.noEnd`) | Approved, and this exact command is allowed in {project} until {time}. |
| applied, prefix | `result.prefix` (`.noEnd`) | Approved, and commands that start with the chosen words are allowed in {project} until {time}. |
| applied, window | `result.window` (`.noEnd`), `result.window.revoke` | Approved. Auto-approve is on in {project} until {time}. Window {windowId}. End it at the terminal with cstan op revoke. |
| forwarded (old host) | `result.forwarded` | Sent to the PM of {project} as approve once. |
| applied, deny | `result.denied` | Denied in {project}. |

`{time}` is the host's `expiresAt` in host time. The `.noEnd` form is used when it is null, so nothing is computed on the phone. Results do not repeat the prefix text.

## Announcements

Each kind has its own title, and the body is `In {project}.` Siri says: "Capstan: Command to approve. In payments-api." There is no command, no agent type and no host text. Time Sensitive starts off for every kind; the user turns kinds on.

| Kind | Title |
|---|---|
| op | Command to approve |
| prompt | Permission prompt to answer |
| design | Design to review |
| message | Message to answer |
| integration | Ready to merge |
| other | Item waiting |

## Settings rows

The three rows:

1. **Notifications → Time Sensitive** (per kind; exists today, every kind off).
   - New footer: "Time Sensitive notifications break through a Focus, so all kinds start off. In CarPlay, Siri reads them aloud when Announce Notifications is on for Capstan. They say the kind and the project, never the text."
2. **Settings → Siri and CarPlay → Decide by voice** (new, off).
   - Footer: "Siri can approve once or deny an Operator proposal after reading the command back to you. Works without Face ID, and on a locked iPhone after its first unlock. Other items are only read out."
3. **Grants by voice** (new, off, disabled while Decide by voice is off).
   - Footer: "Needs Decide by voice. Adds, for Operator proposals only: this exact command or a prefix for the session, and auto-approve for 1 hour or the session. Works without Face ID. Auto-approve asks twice."

The phrases footer: "Say “What’s waiting in Capstan”, “Approve in Capstan”, “Approve once in Capstan” or “Deny in Capstan”." PhoneSettings gains `voiceDecisions` and `voiceGrants`, both Bool, both false by default; a missing key means false.

## Strings

`strings.json` is a flat `{key: text}` object with 130 keys.

**Key groups:**
- `notify.*`: announcements.
- `shortcut.*.title` and `phrase.*`: the App Shortcuts. `{app}` is the application name, written as `\(.applicationName)` in Swift.
- `list.*`, `kind.*`, `age.*`, `where.*`: the listing.
- `frame.command`.
- `prompt.*`: the intents' own prompts: which choice, which prefix number, the second yes.
- `choose.*`.
- `readback.*`.
- `confirm.*`: the fixed confirmation question after the frame.
- `result.*`.
- `refusal.*` and `reason.*`.
- `settings.*`.

**Placeholders:**
- `{project}` and `{host}`.
- `{where}`: a `where.*` value.
- `{n}` and `{number}`.
- `{label}`: a `choose.label.*` value.
- `{kind}` and `{age}`.
- `{command}` and `{prefix}`: untrusted, ITEM_CAP.
- `{reason}`, `{time}`, `{windowId}` and `{app}`.

**Joining lines.** The app joins the read-back line, the frame, any grant lines and the confirm question into one `requestConfirmation` dialog. Each line is its own sentence.

**The check.** `python3 docs/design/plan-156-design-voice/check.py` fails when:
- a key is missing or extra against its contract list;
- any string holds a full-auto word (`/[Ff]ull[ _-]?[Aa]uto|autoAll|approveAll/`);
- an announcement has a placeholder other than `{project}` or names an agent type;
- a fixed line is over LINE_CAP;
- the frame or a named refusal key is missing;
- this note does not state the two caps;
- once `apps/ios/CapstanPhone/Voice/Shortcuts.swift` exists, a `phrase.*` value is not one of its string literals. Only phrases are compared.

**Result now:** 130 keys, 130 in the contract, longest fixed line 120 characters (`result.prefix.noEnd`), no failures. Shortcuts.swift is not present yet, so the phrase check is skipped.

## Rule deviations

- **T3 / T4:** iOS footers are 13 pt, and 13, 15 and 17 sit within 1.2× of each other. These are the platform text styles, as in the iPhone record.
- **K1:** the record's concentric radii (55, 22) and capsule switches.
- **L8:** these are native screens, so the 375/768/1440 px web widths do not apply. 402 pt (Pro) is drawn.
- **A1 (non-text):** the off switch track (a system control) is 1.29:1 (light) and 1.55:1 (dark) against the cell, like the system's own switch. Its knob and the row label carry the state, and VoiceOver says "off".
- **I1 / I4:** no imagery.
- **Package text:**
  - The package lists "the design name" as short text to read. Designs are read as kind, project and age only (the user's safeguard, revision 2).
  - The package lists `truncated` with the limits. It is not decidable at all, by rule 1.

## Design record changes

No DESIGN.md exists, and this package does not own one; none is created. Proposed for when one is written: a **voice surface** beside the iPhone record, with these rules:
- every spoken line is one sentence, with fixed words at most LINE_CAP;
- untrusted text is spoken only verified, whole, printable ASCII and within ITEM_CAP, always inside a fixed frame and before fixed app text;
- a decision by voice always follows the read-back and Siri's yes/no;
- voice decides only Operator proposals;
- grants and auto windows need their own switch, off by default;
- an auto window needs two yeses after its coverage is read;
- notifications carry the kind and the project only;
- no line asks the driver to use the phone.

No token changes.

## Kept, moved, changed, removed

- **Kept:**
  - every settings row;
  - the Time Sensitive default (empty);
  - notification identifiers, categories, threads, sounds and deep links;
  - Face ID for the app itself.
- **Moved:** the plan-155 files, now under the plan-156 names.
- **Changed:**
  - notification titles and bodies on the phone;
  - the Time Sensitive footer;
  - the read-back, now framed;
  - string keys: shortcut phrases are now `phrase.*`; the choice, prefix and second-yes questions are now `prompt.*`.
- **Added:**
  - the Siri and CarPlay section with Decide by voice and Grants by voice;
  - four App Shortcuts;
  - `frame.command`, `refusal.locked`, `refusal.notPaired`, `list.item.unreadable`, `where.unreadable`, `reason.nonAscii` and `reason.project`.
- **Removed:**
  - `list.item.text`: the command is now read through `frame.command`;
  - the prefix text in `result.prefix`;
  - the plan-155 Time Sensitive default (op, prompt).

## Verification

- **Screenshots:** the settings rows (light and dark), the announcements (light and dark), the dialogs and the overview. All were rendered with Playwright from the board sources (static markup, no canvas runtime), and the settings PNGs were looked at.
- **Contrast (computed, not by eye):** `contrast.js` composited each text colour over its ancestors' backgrounds.

  | Board | Elements | Lowest ratio |
  |---|---|---|
  | Settings | 102 | 4.97:1 |
  | Announcements | 64 | 4.97:1 |
  | Dialogs | 239 | 4.97:1 |

  None is under 4.5:1.
- **Fit:**
  - Every footer fits inside the 780 pt phone, with at least 123 pt to spare.
  - The dialogs columns end at 2027 px on the 2080 px board.
- **Strings:** `check.py` passes (above). The Swift literal matcher was tried on a sample Swift line that holds `\(.applicationName)`.
- **Keyboard:** the boards are static, and the focus state (a 3 px ring) is drawn. The Full Keyboard Access order on Settings: back, then each row from top to bottom, then Decide by voice, then Grants by voice (skipped while disabled). This was not traversed on a device.
- **Not verified:**
  - how Siri speaks these lines in CarPlay: pauses around the frame, symbols such as `--`, hyphenated names;
  - whether Siri in CarPlay shortens the lines;
  - whether Siri in CarPlay accepts a spoken number and a second confirmation;
  - whether CarPlay runs `requestConfirmation` at all;
  - SF Pro metrics (Inter stood in);
  - AX text sizes;
  - the SwiftUI build;
  - real dark mode through the OS: dark is drawn with the boards' dark class.

## Self-critique

**Avoid list:**

| Item | Result |
|---|---|
| Default gradient | pass |
| Glassmorphism everywhere | pass |
| Emoji or icon grids | pass |
| Three identical cards | pass: the six notifications are six real kinds |
| Centred hero | pass |
| Everything rounded and soft-shadowed | partial: iOS grouped sections and capsule switches |
| Lorem ipsum | pass |
| Generic Inter on white | pass: Inter stands in for SF Pro |

**Require list:**

| Item | Result |
|---|---|
| At most two typefaces | pass: SF Pro, and SF Mono for commands |
| Spacing scale | pass: 4 pt |
| One accent | pass: rope amber only on the "Siri" labels; switches use the system green, as in the record |
| Deliberate hierarchy | pass |
| Real content | pass |
| Motion only with meaning | pass: none added |
| Empty, loading, error, hover (pressed) and focus states | pass |

**Rules:**

| Group | Result |
|---|---|
| Typography | T1 pass, T2 pass, T3 deviation, T4 deviation, T5 pass, T6 pass, T7 pass, T8 n/a |
| Colour | C1 to C6 pass, C7 n/a |
| Layout | L1 to L4 pass, L5 n/a, L6 pass, L7 pass, L8 deviation, L9 pass |
| Imagery | I1 to I5 n/a, I6 pass |
| Copy | P1 to P6 pass |
| Motion | M1 to M5 pass |
| Components | K1 deviation, K2 to K4 pass, K5 n/a, K6 pass, K7 pass |
| Record | R1 to R7 pass |
| Accessibility | A1 pass for text and a deviation for the off track; A2 pass; A3 pass; A4 pass; A5 n/a; A5b not drawn; A6 pass |

## Open questions

1. **Limited items and voice.** The phone offers approve once for floor, restart and not_checked. Voice keeps them deny only (rule 8), as the PM asked. Do you confirm this?
2. **ntfy announcements.** Prompt announcements come from the ntfy app; their wording is set by the host's ntfy texts, not by this table.
