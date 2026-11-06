# plan-153 design-approve: approve sheet for Operator proposals (iPhone and Mac)

Status: for the user's approval. The user picked **direction B, Decision sheet** (2026-11-05, through pm-1). No code is written in this package.

- **Design (all boards, iPhone and Mac, light and dark):** https://claude.ai/artifact/AnnZSuGkSvwtDo4upEdcXu
- Direction A, Inline choices (not picked): https://claude.ai/artifact/5E3gdf944cMP6UaKvhv3Rg
- Direction B, Decision sheet (picked): https://claude.ai/artifact/QJXzyw9vD8Muvw9czSLSrf
- PNGs: `<board>-<ios|mac>-<light|dark>.png` in this folder (28 files); direction screenshots in `directions/`; `focus-tab-check.png` (Mac scope sheet, focus on the third row after three Tabs).
- `strings.json` (every new fixed string), `contrast.py` and `contrast.csv` (colour pairs), `source/` (the artboard HTML and CSS, the same files as the Design artifact; the artifact names `choices-ios` `Main.dc.html`).

The boards are HTML mockups of native SwiftUI screens. The iPhone follows `docs/design/iphone-app.md` (ItemDetailScreen, op body) and the Mac follows `docs/design/mac-app.md` (InspectorScreen, op body). They reuse those records' tokens, components and rules; this package adds no token. Commands, hashes, ids and times are example data.

## Brief

**Users.** The person who runs Capstan and answers Operator proposals away from the terminal, on the phone or from the Mac app. An expert in their own projects; many short visits, often from a notification. The task that must be fast and safe: read the exact command, choose how far to trust it, and know what was applied.

**Purpose.** Offer the host's `approveChoices` (once; exact or prefix grant for the session; auto-approve window for 1 hour or the session; deny) and send `op approve-as`. The host applies the answer at once as the operator, so the sheet must state the reach of each answer before the tap, and show the host's real result after it.

**Tone.** Calm, exact, unhurried. **Axis scores (1 to 5):** task intensity 4, data density 3, brand expression 2, audience 2 (expert).

**References (traits only).**
- 1Password for Mac and iOS item sharing sheet: a short stepped sheet that names the reach (who, how long) before the share. Used for step 2 and the end texts.
- Apple Settings, Location permission ("Allow once", "While using", "Never"): one decision with plain consequences under each option. Used for the radio cards.
- GitHub branch protection rule editor: a pattern shown verbatim in mono, with the matching rule in a line under it. Used for the prefix rows and the whole-words line.
- macOS Screen Time "Ask for more time" (15 minutes, 1 hour, all day): time-boxed grants with an explicit end. Used for the window choice.
- Apple Wallet payment confirm: a final confirm only for the widest action. Used for the auto confirm.

**Constraints.** iOS 26 and macOS 26 SwiftUI (`.sheet` with `.large` detent, `NavigationStack` inside the sheet for steps on iPhone; a Mac sheet with in-place steps; `.confirmationDialog`/alert for the auto confirm). SF Pro and SF Mono (Inter and IBM Plex Mono stand in). Light and dark. WCAG 2.2 AA; Dynamic Type to AX5 on iPhone; Full Keyboard Access and Mac keyboard. No host text in a notification, widget, Live Activity or menu bar. No full auto from a remote client. No number the host does not send.

**What was read first.** 1. DESIGN.md: none; `cstan design guide rules` read; no `docs/design/directions.md` or `rules.md`. 2. Design system: Capstan (Claude Design) README and tokens; the approved iPhone and Mac records override it on app surfaces (user decisions 2026-10-28 and 2026-10-30). 3. Brand: rope amber `#8a521c` light, `#e0a45e` dark. 4. Current UI: approved iPhone boards 03, 04, 05 and Mac 01, 03, 04 (option rows, untrusted mono blocks, outcome banners, deny sheet). 5. Platform: as above. 6. Content extremes: commands up to 4096 bytes; up to 3 prefixes, shortest first, the longest can exceed one line; host messages of a refusal; ids such as `win-7`. 7. Users and tasks: above. 8. Accessibility: WCAG 2.2 AA plus the iOS settings in the iPhone record. 9. Scope: design only, the seven boards the plan names. The ui-ux-pro-max design-system query suggested an OLED dark style, a green accent and JetBrains Mono; not used, the approved app tokens win.

## Directions offered

| | A Inline choices | B Decision sheet (picked) |
|---|---|---|
| Catalog | Workbench + Conversation | Briefing + Plain Speech |
| Where the answers are | rows in the detail under the command | one Answer… button opens a guided sheet |
| Steps | each row opens one step | step 1 answer, step 2 scope or window |
| Density | medium | low |
| Audience | expert | broader |

They differ on data density and audience. The user chose B.

## Flow (B)

```
Detail (op item with approveChoices, command verified and whole)
├─ Deny…  → the approved deny sheet (iPhone board 04, Mac 03), unchanged
└─ Answer… → sheet, step 1 "Your answer": command (untrusted, mono) + radio cards
    ├─ Approve once        → primary "Approve once" applies (approve-as once)
    ├─ Approve + allow similar for the session → Continue → step 2 "Allow similar commands"
    │     This exact command | Commands that start with: <prefix> (one row per offered prefix)
    │     → primary "Approve and allow" (approve-as exact | prefix --prefix <raw>)
    └─ Auto-approve all    → Continue → step 2 "Auto-approve all"
          For 1 hour | For this session + coverage text + end text → Continue
          → confirm "Open an auto-approve window?" (Cancel default) → approve-as auto-hour | auto-session
Result: the sheet closes; the detail shows the applied result. Refusal: shown in the sheet above the primary.
```

Taps after Answer…: once 2 (card, Approve once); similar 4 (card, Continue, scope, Approve and allow); auto 5 (card, Continue, window, Continue, confirm). The plan's risk note asked for two taps per choice (choice, then scope or window, then confirm only for auto); B adds the Continue tap per step, which the user accepted by picking B. Only the auto window has a confirm.

Rows appear only for what `approveChoices` offers: `exact` false hides This exact command, `prefixes` empty hides the prefix rows (and Approve + allow similar when neither is offered), `auto` empty hides Auto-approve all. No board offers full auto.

## Boards and states

| Board | iPhone (`<board>-ios-*`) | Mac (`<board>-mac-*`) |
|---|---|---|
| choices | 1 detail with Deny… and Answer… (focus on Answer…); 2 step 1 with nothing chosen, primary disabled "Choose an answer"; 3 Approve once chosen, primary "Approve once", no step count; 4 Allow similar chosen with keyboard focus on the card, "1 of 2", primary Continue | the window with the inspector behind a sheet: step 1, Allow similar chosen, focus on Continue |
| scope | 1 step 2, the second prefix chosen; 2 a long prefix wrapped whole in its box with "One value, shown whole"; 3 Dynamic Type AX3: rows stack, the long prefix wraps whole | step 2 with the long prefix, wrapped inside its box |
| auto | 1 step 2, For 1 hour chosen, coverage and end texts; 2 For this session chosen; 3 the confirm over the sheet, Cancel focused | step 2 (1 hour) with the confirm alert over it, Cancel focused |
| limited | 1 floor with the floor entry; 2 restart; 3 not_checked: step 1 shows only Approve once, with "Only Approve once and Deny are offered." and the reason; Deny… stays on the detail; 4 truncated: Deny only, no Answer…; 5 text_mismatch: Deny only | a gallery: floor, restart, not_checked sheets; truncated and text_mismatch inspectors with Deny… only; the floor inspector before the sheet |
| result | 1 sending (primary disabled with a spinner); 2 once; 3 grant: the prefix (or "This exact command") and "Ends 16:58"; 4 window: id `win-7`, "Ends 14:41", the revoke line; 5 denied | gallery: sending, once, grant, window, denied in the inspector |
| refusal | 1 busy (yellow, the sheet stays, primary live to try again); 2 not_waiting (grey, the sheet closes, the detail has no actions); 3 hash_mismatch (red, "Show the new command", primary disabled); 4 needs_terminal with the host message in an untrusted block (primary disabled; Back returns to step 1 where Approve once may still be offered); 5 invalid_request (primary disabled). Each says "Nothing was approved." and names the code | gallery: the same five in the Mac sheet |
| oldhost | 1 the approved detail with Deny… and Send approval to PM, unchanged; 2 the approved forward confirm | the approved inspector with Send approval to PM, unchanged |

States across the boards: empty (step 1 with nothing chosen), loading (sending), error (every refusal), hover (Mac rows and buttons use the approved `fill2` hover; iPhone uses pressed), focus (3 px ring in `focus` on every control, shown on choices, auto and the Tab check), disabled (primary before a choice and after a refusal).

**Result details.** `approval {choice, grantId, windowId, expiresAt}` drives the result: once shows no grant and no window; exact and prefix show the grant text the person chose (the prefix in mono, or This exact command) and `Ends <expiresAt>` in host time; a window shows `Window <windowId>` and `Ends <expiresAt>`. A null `expiresAt` hides the Ends line; nothing is computed on the device. The PM line ("The PM was told and does not ask you again.") follows every applied result.

**Prefix rule (acceptance 5).** A prefix is shown whole, in mono, in its own bordered box, exactly the bytes sent (never trimmed, split, re-joined or cut). A prefix longer than one line wraps by character inside its box (SwiftUI: one `Text` with `.lineBreakMode(.byCharWrapping)` and no line limit), so it never reflows into the words around it, and gets the "One value, shown whole" caption. VoiceOver reads it as one element, "Prefix, one value", then the text.

## What differs between iPhone and Mac, and why

| iPhone | Mac | Why |
|---|---|---|
| Large-detent sheet with a nav row (Cancel or Back, title, step count) and one full-width primary at the bottom | Centred sheet over the window, title and step count on top, Back or Cancel and the primary right-aligned | HIG sheet patterns; thumb reach on the phone, button order on the Mac |
| Steps push inside the sheet (NavigationStack); Back in the nav row | Steps replace the sheet content in place; Back is a button left of the primary | No navigation bar in a Mac sheet |
| Auto confirm: a small sheet over the step, Cancel first | Auto confirm: an alert over the sheet, Cancel is the default button | Platform confirm idioms; both default to Cancel |
| Result in the detail above the meta | Result in the inspector above the meta | Where each app already shows outcomes (iPhone board 05, Mac 04) |
| 17 pt titles, 13 pt details, 50 pt buttons, 56 pt minimum card height | 13 pt titles, 12 pt details, 28 pt buttons | Platform text styles (the approved records) |
| Dynamic Type to AX5: cards stack, the step count stays, the prefix wraps whole, the primary grows (drawn at AX3) | Follows the system text size; the sheet scrolls | iOS Dynamic Type; macOS text size |
| Keyboard: Full Keyboard Access order below | Return = primary, Esc = Cancel/Back, ↑↓ move the radio choice, ⌘↩ = Answer… | Mac is keyboard-first |
| Limited gallery shown as five phones | Shown as a panel gallery (sheet or inspector parts) | Space; the sheet is the same component |

## Dynamic Type and keyboard order

- **Dynamic Type (iPhone).** All text uses text styles; at accessibility sizes the sheet title moves under the nav row, the prefix wraps by character inside its box and the primary button grows to fit (scope board, phone 3, AX3). The sheet always scrolls; the primary stays pinned at the bottom with a solid background.
- **Keyboard order (iPhone, Full Keyboard Access):** detail: Waiting → More actions → Deny… → Answer…; sheet step 1: Cancel → Approve once → Approve + allow similar → Auto-approve all → primary; step 2: Back → each row in order → primary; auto confirm: Cancel (initial focus) → Open window and approve. The detail behind a sheet is inert.
- **Keyboard order (Mac):** sheet: rows in order → Back/Cancel → primary (verified by Tab traversal on the rendered scope board: the window behind the sheet is skipped). Disabled primaries are not focusable.

## Strings (`strings.json`)

A flat object `{key: string}` with every new fixed string of the boards; the app packages copy it into `ApproveStrings.swift` and their drift scripts check this key set and the values. Placeholders: `{n}` and `{total}` (step.of), `{windowId}` (result.window.id), `{time}` (result.ends, host time from `expiresAt`). Strings the apps already have and this design leaves unchanged (the detail header, meta labels, untrusted block headers, the deny sheet, the old forward confirm) are not in the file.

The stable key set (61 keys):

`op.title`, `choices.heading`, `choices.applied`, `answer.action`, `deny.action`, `step.of`, `choices.none`, `choice.once.title`, `choice.once.detail`, `choice.similar.title`, `choice.similar.detail`, `choice.auto.title`, `choice.auto.detail`, `once.confirm.action`, `action.cancel`, `action.back`, `scope.title`, `scope.exact.title`, `scope.exact.detail`, `scope.prefix.title`, `scope.prefix.wrapped`, `scope.rule`, `scope.length`, `scope.action`, `auto.title`, `auto.hour.title`, `auto.hour.end`, `auto.session.title`, `auto.session.end`, `auto.end.both`, `auto.coverage.heading`, `auto.coverage`, `auto.next`, `auto.confirm.title`, `auto.confirm.action`, `limited.lead`, `limited.floor`, `limited.floor.entry`, `limited.restart`, `limited.truncated`, `limited.not_checked`, `limited.text_mismatch`, `result.sending`, `result.once`, `result.grant`, `result.window`, `result.window.id`, `result.ends`, `result.window.revoke`, `result.denied`, `result.pm`, `refusal.nothing`, `refusal.busy`, `refusal.not_waiting`, `refusal.hash_mismatch`, `refusal.hash_mismatch.action`, `refusal.needs_terminal`, `refusal.invalid_request`, `refusal.host_says`, `oldhost.action`, `oldhost.note`

Meaning checks: `auto.coverage` says the window approves every Operator command and every worker permission prompt, pushes and deletes included, except the floor list (deletes or writes outside the project, a controller restart); scripts and shells are approved unchecked; web fetch, MCP and other tool prompts are answered without a floor check. `auto.hour.end` says up to 1 hour, less when the host's limit is shorter; `auto.session.end` says until the host's configured limit; `auto.end.both` says both end when the controller restarts or the window is revoked with `cstan op revoke` at the terminal. No string holds a number the host does not send (the 4096 in `limited.truncated` is the protocol's feed text cap).

## Contrast

`python3 docs/design/plan-153-design-approve/contrast.py` computes each pair from the approved tokens (alpha layers composited, top first) and writes `contrast.csv`. Text needs 4.5:1, non-text control boundaries, focus rings and selection marks 3:1; disabled controls are listed and exempt.

| id | app | use | fg over bg | kind | min | light | dark |
|---|---|---|---|---|---|---|---|
| T01 | ios | Sheet title, card titles, command text | `label` over `cell` | text | 4.5 | 16.39 | 14.52 |
| T02 | ios | Card details, notes, step count | `label2` over `cell` | text | 4.5 | 6.88 | 7.45 |
| T03 | ios | Detail notes and meta labels on the ground | `label2` over `ground` | text | 4.5 | 5.97 | 8.28 |
| T04 | ios | Back and Cancel (plain accent buttons) | `accent` over `cell` | text | 4.5 | 6.36 | 7.76 |
| T05 | ios | Primary button text (Answer…, Continue, Approve…) | `on-accent` over `accent` | text | 4.5 | 6.36 | 8.51 |
| T06 | ios | Selected card title | `label` over `accent-tint over cell` | text | 4.5 | 13.57 | 10.73 |
| T07 | ios | Selected card detail | `label2` over `accent-tint over cell` | text | 4.5 | 5.70 | 5.50 |
| T08 | ios | Prefix value in its box (selected card) | `label` over `mono-bg over accent-tint over cell` | text | 4.5 | 12.47 | 9.20 |
| T09 | ios | One value, shown whole (selected card) | `label2` over `accent-tint over cell` | text | 4.5 | 5.70 | 5.50 |
| T10 | ios | Deny… button text | `red` over `fill2 over ground` | text | 4.5 | 5.58 | 7.60 |
| T11 | ios | Limit line and coverage text | `label` over `fill2 over cell` | text | 4.5 | 14.01 | 10.79 |
| T12 | ios | Floor entry label, end text in coverage | `label2` over `fill2 over cell` | text | 4.5 | 5.88 | 5.54 |
| T13 | ios | Result lead on green tint | `label` over `green-tint over ground` | text | 4.5 | 12.37 | 12.94 |
| T14 | ios | Result small print on green tint | `label2` over `green-tint over ground` | text | 4.5 | 5.19 | 6.63 |
| T15 | ios | Refusal lead on red tint | `label` over `red-tint over cell` | text | 4.5 | 14.15 | 12.04 |
| T16 | ios | Refusal small print on red tint | `label2` over `red-tint over cell` | text | 4.5 | 5.94 | 6.18 |
| T17 | ios | busy lead on yellow tint | `label` over `yellow-tint over cell` | text | 4.5 | 14.21 | 11.09 |
| T18 | ios | busy small print on yellow tint | `label2` over `yellow-tint over cell` | text | 4.5 | 5.96 | 5.69 |
| T19 | ios | Host message in its block on red tint | `label` over `mono-bg over red-tint over cell` | text | 4.5 | 13.00 | 10.35 |
| T20 | ios | Denied result on grey | `label2` over `fill2 over ground` | text | 4.5 | 5.12 | 6.37 |
| U01 | ios | Radio ring, not chosen | `label2` over `cell` | ui | 3.0 | 6.88 | 7.45 |
| U02 | ios | Selection border of the chosen card | `accent` over `cell` | ui | 3.0 | 6.36 | 7.76 |
| U03 | ios | Radio dot of the chosen card | `accent` over `accent-tint over cell` | ui | 3.0 | 5.26 | 5.73 |
| U04 | ios | Focus ring on the sheet | `focus` over `cell` | ui | 3.0 | 6.48 | 7.69 |
| U05 | ios | Focus ring on the detail ground | `focus` over `ground` | ui | 3.0 | 5.62 | 8.54 |
| U06 | ios | Primary button edge (accent fill) | `accent` over `cell` | ui | 3.0 | 6.36 | 7.76 |
| D01 | ios | Disabled primary (Choose an answer, after a refusal) | `label2` over `fill2 over cell` | disabled | exempt | 5.88 | 5.54 |
| T21 | mac | Sheet title, radio titles, command text | `label` over `win` | text | 4.5 | 15.14 | 14.52 |
| T22 | mac | Radio details, notes, step count | `label2` over `win` | text | 4.5 | 6.36 | 7.45 |
| T23 | mac | Radio row title on its fill | `label` over `fill over win` | text | 4.5 | 14.03 | 13.03 |
| T24 | mac | Radio row detail on its fill | `label2` over `fill over win` | text | 4.5 | 5.89 | 6.68 |
| T25 | mac | Primary button text | `on-accent` over `accent` | text | 4.5 | 6.36 | 8.51 |
| T26 | mac | Secondary button text (Back, Cancel) | `label` over `fill2 over win` | text | 4.5 | 12.97 | 11.16 |
| T27 | mac | Deny… button text | `red` over `fill2 over win` | text | 4.5 | 5.94 | 6.82 |
| T28 | mac | Selected radio detail | `label2` over `accent-tint over win` | text | 4.5 | 5.29 | 5.50 |
| T29 | mac | Prefix value in its box (selected row) | `label` over `mono-bg over accent-tint over win` | text | 4.5 | 11.60 | 9.20 |
| T30 | mac | Result small print on green tint | `label2` over `green-tint over win` | text | 4.5 | 5.51 | 5.81 |
| T31 | mac | Refusal small print on red tint | `label2` over `red-tint over win` | text | 4.5 | 5.50 | 6.18 |
| T32 | mac | busy small print on yellow tint | `label2` over `yellow-tint over win` | text | 4.5 | 5.53 | 5.69 |
| T33 | mac | Limit line small print on grey | `label2` over `fill2 over win` | text | 4.5 | 5.44 | 5.72 |
| U07 | mac | Radio ring, not chosen | `label2` over `fill over win` | ui | 3.0 | 5.89 | 6.68 |
| U08 | mac | Selection border of the chosen row | `accent` over `win` | ui | 3.0 | 5.87 | 7.76 |
| U09 | mac | Radio dot of the chosen row | `accent` over `accent-tint over win` | ui | 3.0 | 4.89 | 5.73 |
| U10 | mac | Focus ring | `focus` over `win` | ui | 3.0 | 5.99 | 7.69 |
| D02 | mac | Disabled primary (after a refusal, while sending) | `label2` over `fill2 over win` | disabled | exempt | 5.44 | 5.72 |

A second, computed-style check ran in the rendered boards (Playwright, every text element over its composited background, all 28 PNG pages): every element passes; the lowest is 4.82:1 (the sidebar badge of the approved Mac window, dark), and the lowest on the new parts is 5.08:1.

## Plan deviations

- **truncated is Deny only.** The plan's design text lists truncated with Approve once and Deny. The architect (architect-61, 2026-11-05) narrowed it: a command longer than the app can show is never approved from a remote client, not even once and not by a forward, because the forward now means the user's approve once. The limited board shows Deny… only, with the line that the first 4096 bytes are shown and it can only be approved at the terminal. This matches today's iPhone rule; on the Mac it removes today's forward of a cut command.
- **Taps.** B uses a Continue step, so similar takes 4 taps and auto 5 after Answer… (see Flow); the user chose B over A, which met the two-tap note.

## Rule deviations

- **Design system vs the app records: decided by the user** (2026-10-28, 2026-10-30). Glass on floating layers, concentric radii and SF Pro replace the web system's flat 4 px look; the accent, voice and mono rule are kept.
- **T3 / T4:** iOS 13/15/17 and Mac 12/13 text styles sit within 1.2× of each other; these are the platform text styles, as in both records.
- **K1:** the radius family is the records' concentric set (sheet 38/47, cards 22 on iPhone and 12 on the Mac, prefix boxes 8 and 6, capsule buttons).
- **L8:** native app screens; no 375/768/1440 web layout. iPhone 402 pt and AX3 are drawn; the Mac sheet at the standard window. 768 and 1440 are not applicable.
- **I1/I4:** no imagery; not applicable.

## Design record changes

No DESIGN.md exists and this package does not own one; none is created. Proposed additions for when one is written, beside the iPhone and Mac records: a **decision sheet** component (step header with step count, radio cards with a title and a consequence line, one primary that names the result); a **prefix value** component (mono, bordered, wraps by character, "One value, shown whole"); a **limit line** (grey, info glyph, lead plus reason, optional floor entry); the rule that the widest grant (an auto window) always has a confirm whose default is Cancel; and the forbidden moves below. No token changes.

## Forbidden moves (this surface)

- No full auto choice, and no answer by swipe, notification action, widget, Live Activity or menu bar.
- No host text (commands, prefixes, floor entries, host messages) outside an untrusted mono block or the prefix box; never in a notification, widget, Live Activity or the menu bar.
- No prefix trimmed, cut, ellipsised or reflowed into other words.
- No end time the host did not send.
- No approve of a command the app cannot show whole.

## Verification

- **Screenshots:** 28 PNGs, one per board, app and theme, rendered with Playwright from the artboard source (a local stub expands the light and dark loop; `source/` holds the same files).
- **Contrast:** `contrast.py` (90 rows, none below threshold) and the computed-style check above.
- **Keyboard:** Tab traversal on choices (iPhone), auto (iPhone) and scope (Mac) boards; every stop shows a 3 px solid ring. The first run found the Mac window behind the sheet and the disabled primary in the Tab order; both fixed (the window is inert under a sheet; disabled primaries are `disabled`).
- **Not verified:** dark mode via the OS media query (the boards switch by their light/dark class, as in the approved records); real Liquid Glass; SF Pro metrics (Inter stood in); AX5 (AX3 is drawn); VoiceOver and Full Keyboard Access on a device; the SwiftUI build; backgrounds behind a sheet's scrim were not part of the computed check.

## Self-critique (Avoid / Require)

Avoid: purple-blue or default gradient **pass**; glassmorphism everywhere **pass** (glass only where the records allow; sheets with host text are opaque); emoji or icon grids **pass**; three identical cards in a row **pass** (three radio cards are three different answers, stacked, each with its own consequence); centred hero with vague copy **pass**; everything rounded and soft-shadowed **partial** (platform capsule buttons and concentric radii, shadow only on sheets); lorem ipsum **pass**; generic Inter on white **pass** (SF Pro is the real face; Inter only stands in).

Require: at most two typefaces **pass** (SF Pro, SF Mono); spacing scale **pass** (4 pt); one accent **pass** (rope amber for the primary, the chosen card and plain nav buttons; state colours only on results and refusals); deliberate hierarchy **pass** (one primary per step that names the result; Deny kept apart on the detail); real content **pass**; motion with meaning **pass** (system sheet push between steps, spinner while sending; Reduce Motion uses the system cross-fade); empty, loading, error, hover, focus states **pass** (choices 2, result 1, refusal, Mac hover per record, focus rings).

Rules: T1 pass · T2 pass · T3 deviation · T4 deviation · T5 pass · T6 pass · T7 pass (times and ids tabular) · T8 n/a · C1 pass · C2 pass · C3 pass (computed) · C4 pass (each result and refusal has a glyph and words) · C5 pass · C6 pass · C7 n/a · L1 pass · L2 pass · L3 pass · L4 pass · L5 pass (the primary is full width on the phone, right-aligned on the Mac; the limit line breaks the card rhythm) · L6 pass · L7 pass · L8 deviation · L9 pass · I1 n/a · I2 pass · I3 pass · I4 n/a · I5 n/a · I6 pass · P1 pass · P2 pass · P3 pass (Approve once, Approve and allow, Open window and approve) · P4 pass · P5 pass · P6 pass · M1 pass · M2 pass · M3 pass · M4 pass · M5 pass · K1 deviation · K2 pass · K3 pass · K4 n/a (no form fields beyond the approved deny note) · K5 pass (prefix overflow: wraps whole) · K6 pass · K7 pass · R1 pass (no new token) · R2 pass · R3 pass · R4 pass · R5 n/a · R6 pass · R7 pass · A1 pass · A2 pass (cards 56 pt, buttons 50 pt; Mac 28 px buttons, 36 px rows) · A3 pass · A4 pass · A5 n/a (native) · A5b pass (AX3 drawn) · A6 pass.

## Open questions

1. **Step count for once.** Approve once applies from step 1 with no step count shown; similar and auto show "1 of 2". Is that clear enough, or should step 1 never show a count?
2. **needs_terminal after a floor change.** The sheet keeps the person on step 2 with the host's message and a disabled primary; Back leads to step 1, where Approve once may still be offered. Should the app instead return to step 1 by itself?
3. **Window end for a session.** `expiresAt` for auto-session is the host's configured limit; if the host sends null, the result shows no Ends line. Is a null expected?
