# PM card, stuck alert and screen peek on iPhone and Mac (plan-152, design-pm)

Status: **for the user's approval.** One decision covers all three boards: card and stuck (built by plan-152's app-ios-pm and app-mac-pm) and peek (built later by plan-151's peek-apps, so plan-151 needs no second approval). Design only; no implementation code.

- **Design (Claude Design, 6 artboards, each light and dark):** https://claude.ai/artifact/21EeWzLFgF3abEbZWA1KeD
- PNGs, the build spec: `docs/design/plan-152-design-pm/<board>-<ios|mac>-<light|dark>.png` for board `card`, `stuck` and `peek` (12 files).
- Contrast: `contrast.py` (standard library) and `contrast.csv` in the same folder. `python3 docs/design/plan-152-design-pm/contrast.py` prints every pair; `--point <png> fx fy bx by` measures any extra pair.
- Board sources (the `.dc.html` artboards and their CSS, for revisions): `docs/design/plan-152-design-pm/source/`.

The iPhone boards follow `docs/design/iphone-app.md` (direction A, Glass Inbox). The Mac boards follow direction A (Glass Workbench) and `docs/design/mac-app.md`, beside plan-149's boards, which are unchanged. The card links to Messages; it does not redesign it. Hosts, projects, agents and messages are example data.

## Brief

**Users and task.** The person who runs Capstan and checks it from a phone or a Mac in short visits. The task that must be fast: tell at a glance whether the project's PM is moving, what it last said, whether it has read what you sent, and who or what it is waiting on; when it is stuck, notice and act (message it or pause the project).

**Purpose.** Show the run item's new `pm` member (plan-152 host-pm-card and host-pm-stuck) on the project overview, raise one alert per stuck episode, and (plan-151) let the operator look at the PM's terminal on request.

**Tone.** Calm, exact, native. **Axes (1 to 5):** task intensity 4 (operate), data density 3, brand expression 2, audience 2 (expert). Unchanged from the approved app records.

**Directions.** Not offered again. The package fixes iPhone direction A and Mac direction A with their approved tokens, as plan-149's design packages did; the PM was told when work started.

**References (traits only).**
- GitHub's pull request merge box: a status header, then "waiting on" checks as rows with a count and an age, each opening its list. Used for the Waiting on rows.
- Apple Messages: the delivery word under the last message you sent (Delivered, Read). Used for the "You sent" line.
- Xcode's build status in the toolbar: one word with a glyph and a time, never colour alone. Used for the activity line.
- Apple Mail's "unable to connect" banner: a cause, a time and the next step, above the content it affects. Used for the stuck banner and the offline state.
- Terminal's Inspector and tmux's capture-pane: a fixed snapshot of a screen, with when it was taken. Used for the peek sheet.

**What was read first (design guide order).**
1. DESIGN.md: none found. `docs/design/directions.md` and `docs/design/rules.md`: none. `cstan design guide rules` read.
2. Design system: Capstan (Claude Design) is installed on the canvas. The app records override its web values (no glass, 4 px radii, IBM Plex) on app surfaces by the user's decision of 2026-10-28.
3. Brand: rope amber `#8a521c` (light) and `#e0a45e` (dark).
4. Current UI: iPhone `ProjectOverviewScreen.swift` (title, status card, Waiting for you, Agents, Plans, outcomes, the existing Pause sheet) and Mac `OverviewView.swift` (section blocks); approved iPhone boards 09, 11, 15 and 17; Mac boards 12 and 16 and plan-149's 16r delivery words.
5. Platform: SwiftUI on iOS 26 and macOS 26; SF Symbols; light and dark; no web views for host text.
6. Content extremes: the PM's last message and your last message are untrusted text, cut at 300 bytes by the host (`fromUser.body`); waiting kinds are at most 5 with counts that can reach hundreds; ages from seconds to days; peek is 1 to 60 lines (40 by default) of up to 240 characters, at most 8 KiB.
7. Users and tasks: above.
8. Accessibility: WCAG 2.2 AA, Dynamic Type through AX5, Full Keyboard Access, VoiceOver, Reduce Motion.
9. Scope: three boards on two apps and this note; estimate 4 hours.

The ui-ux-pro-max query suggested a dark-only OLED style with a green accent and Fira; it was not used, because the approved app tokens win. Its UX rules were applied: errors say the cause and the next step, colour is never the only signal, every sheet has an escape, loading has a shape.

## Boards and states

### card: the PM card on the project overview

Where: iPhone `ProjectOverviewScreen`, a "Project manager" section right after the status card and before Waiting for you. Mac `OverviewView`, a "Project manager" block at the top of the overview, before the existing sections.

Content, top to bottom:
- **Header:** the PM id (mono), the activity as a word with a glyph (Working: filled dot, blue; Idle: ring; Blocked: no-entry glyph, red; Done: check, green; Unknown: question mark), and "since 11:58 · 23 min" from `activitySince`.
- **From the PM:** the newest `messages` item from the PM, two lines of untrusted mono, with its time. Opens the PM conversation (Messages, plan-149's thread).
- **You sent:** `fromUser` (or the app's own sent log, newest wins), two lines of untrusted mono and the delivery line: "Queued: the PM gets it when it is free", "Delivered to the PM · 12:03", "Read by the PM · 12:05", "Not delivered (expired)" (or cancelled, skipped, failed), each with a glyph; Not delivered is red with a triangle. Opens the same conversation.
- **Waiting on:** one row per kind with a count above zero, in host order: Worker reports, Reviews, Verify runs, Plan reviews, You. Each shows the count and the oldest age ("3 · oldest 2 h 10 min"; "oldest" only when the count is above 1). Worker reports, Reviews, Verify runs and Plan reviews open Plans for the project; You opens Waiting for you filtered to the project. A kind the app does not know is shown with the host's name for it and no link.

States (both apps): normal; **pressed** (iPhone) and **hover** (Mac); **focus** (3 px ring); **no PM** ("No PM is active on payments-api, so nobody routes work. The card fills in when a PM registers."); **host offline** (the existing reconnect banner, the card keeps its 12:31 values with a grey "Last known · 12:31" pill, "Working at 12:31", and the waiting heading says "at 12:31"); **stale** (host live but the card's last refresh failed: yellow "Stale · as of 12:38" pill, pull to refresh on iPhone, Refresh and ⌘R on Mac); **loading** (first snapshot: a skeleton shaped like the card and "Reading build-host…"); **nothing yet** (no message either way, nobody waits: "No message to you yet.", "Nothing yet. Write to pm-1 in Messages.", "Nobody waits on anything."); **largest text** (iPhone AX3) and **compact window** (Mac, one column).

### stuck: the stuck banner and the alert

- **Banner,** at the top of the PM card (inside it, so it sits with the facts it explains), yellow tint with a warning glyph, title "pm-1 looks stuck" and one line per reason:
  - unread mail: "It is idle while your message has waited unread for 17 min, since 12:14."
  - no progress: "It shows as working but has made no progress for 22 min, since 12:09."
  - stopped at a prompt: "It has been stopped at a prompt for 15 min, since 12:16. A prompt it relayed waits in Waiting for you."
  - An unknown reason reads "pm-1 looks stuck since 12:09." with the same actions (text only, not drawn).
- **Actions:** "PM messages" (opens the PM conversation) and "Pause project…" (red outline; opens the existing pause sheet, Cancel has focus, the reason field is pre-filled "pm-1 looks stuck" and can be cleared).
- **Variant with Peek,** labelled "plan-151 · not built in plan-152": a third action "Peek" before the others.
- **Alert-false episodes** show the same banner and post no notification. **Recovered:** the banner goes; nothing else is shown.
- **Notification:** title "PM looks stuck", the project name as subtitle, **no body**, one per episode, removed on recovery. iPhone Lock Screen (beside an existing "Command proposal · payments-api" notification, for comparison) and Mac Notification Center.
- **Mac sidebar:** the project row gets a yellow ring and the word "PM stuck" (from the run item, no host text). **Menu bar:** only the existing count. **Widgets and Live Activity:** unchanged (not drawn: nothing is added to them).

### peek: the PM's screen on request (plan-151 builds it)

- **Entry:** a "Peek at the screen" row in the PM card (iPhone) and a "Peek" button in the card header (Mac), plus the Peek action on the stuck banner.
- **Sheet:** iPhone large detent with a grabber and Done; Mac a 760 pt sheet with Refresh (⌘R) and Done (default, Esc). Header: "pm-1 screen", a yellow "Not verified" pill, "Captured 12:31:07", "40 lines" (Mac) and "3 values masked". The text is a flat untrusted mono block, "Terminal of pm-1 · untrusted text · scrolls", wrapped, never linked or styled; `[masked]` spans are outlined chips. Footnote: "Masking is best effort. This text is not kept: it goes when you close the sheet or leave the app."
- **States:** off (`peek_disabled`: what peek is, the `[remote]` / `peek = true` lines for the project's config on the host, `peek_patterns`, Try again); loading (one read, spinner and skeleton); text; rate limited (`rate_limited`: the old text stays, "Too many peeks. build-host allows 6 a minute and one every 5 s. Refresh works again in 4 s." and a disabled "Refresh in 4 s" that counts down); no PM (`no_active_pm`); no pane (`no_pane`); refused (`forbidden`, `unauthorized`: red title, "Only the host's operator may peek, and this iPhone's key is not allowed (forbidden)."); older host (unsupported: "Update Capstan on build-host"); no answer (Retry).
- **Dropped when closed:** the text lives only in the sheet; closing it, leaving the app or locking the phone drops it. Nothing goes to a notification, log, widget, Live Activity or store.

## What differs between iPhone and Mac, and why

| | iPhone | Mac | Why |
|---|---|---|---|
| Card layout | One column: header, From the PM, You sent, Waiting on | Two columns (3:2): messages left, Waiting on right; one column in a compact window | The Mac overview is wide; the phone is one column (iPhone record) |
| Row affordance | Rows with a chevron, pressed state, 44 pt | Rows with a chevron, hover state, 28 pt; "Open conversation ›" on messages | Touch vs pointer; HIG target sizes |
| Text sizes | Body 17, subheadline 15, footnote 13, mono 13 | Body 13, caption 12, mono 12 | iOS vs macOS text styles (both records) |
| Stale refresh | Pull to refresh | Refresh button and ⌘R | Platform idiom |
| Banner buttons | Full-width capsules, 44 pt, stacked when they do not fit | Inline small buttons | Thumb reach vs pointer |
| Stuck elsewhere | Lock Screen notification only | Notification, plus "PM stuck" on the sidebar project row | The Mac sidebar lists projects; the phone has no persistent list on screen |
| Peek container | Large-detent sheet, wraps at about 44 characters | 760 pt sheet, about 100 characters a line | A terminal needs width; a popover is too small on both |
| Peek refresh | Button at the bottom | Button with ⌘R; Done is the default | Keyboard |
| Pause project | The existing iPhone pause sheet (medium detent) | The existing Mac confirm sheet (board 03 pattern) | Reuse; no new destructive flow |

## Host text on glanceable surfaces

No board puts host text in a notification, widget, Live Activity or the menu bar. The stuck notification is a fixed title plus the project name (from the app's own project list). The Mac menu bar keeps only the count. Widgets and the Live Activity are unchanged. The PM's messages and the peek text appear only inside the app, as untrusted mono.

## Dynamic Type and keyboard

- **Dynamic Type (iPhone, card-ios phone 2, AX3):** the header stays two lines; message previews stay two lines at the larger size; Waiting on rows stack the value under the label with the chevron spanning both; the banner's buttons stack one per row (not drawn). Mac follows the system text size through the same text styles.
- **Keyboard order (Tab, measured in the mockups; every stop shows a 3 px ring):**
  - iPhone (Full Keyboard Access): back → More actions → (stuck banner: PM messages → Pause project…) → From the PM → You sent → Waiting on rows in order → tab bar. Pause sheet: initial focus on Cancel; Reason → Cancel → Pause project. Peek sheet: Done → Refresh (or Try again).
  - Mac: sidebar → toolbar (Message PM, More) → stuck banner (PM messages, Pause project…) → From the PM → You sent → Waiting on rows → the rest of the overview. Peek sheet traps focus: Refresh → Done (default); Esc closes.
  - The first run put the Mac toolbar after the content and let Tab reach the window behind the peek sheet; both were fixed in the mockups before the PNGs were taken.

## Tokens

No new tokens. Mac: the colour roles, type steps, 4 px spacing scale, concentric radii and motion of `mac-app.md` (as copied in plan-149's `base.css`). iPhone: those roles plus `ground` and `cell`, the iOS type steps (34, 22, 17, 15, 13, 11 for tab labels) and the radii of `iphone-app.md`. The banner uses `yellow-tint` with `label` text; "Not verified" uses `yellow` on `yellow-tint`; masked spans use `fill2` with a `label2` outline.

Motion: the banner and the card's values change with the 250 ms state change; the "since" text updates without motion; the peek spinner turns only while a read is open. Under Reduce Motion the banner appears without a slide and the spinner becomes "Reading…" text.

## Verification

- **Screenshots:** 12 PNGs (each board, each app, light and dark) rendered with Playwright from the artboard source, with a local stand-in for the canvas runtime that expands the light and dark loop.
- **Contrast:** computed, not judged. For every visible text and control element in every PNG (918 elements), a script took the element's text box, used the most common colour in it as the background and the pixel of highest contrast as the foreground, wrote both points to `contrast.csv`, and cross-checked the foreground against the element's computed CSS colour (the same ratio for every row). `contrast.py` reads those points from the PNGs: 918 pairs, all at least 4.5:1, lowest 4.69:1. The first run found one failure (the peek sheet's Done on dark took the browser's default button grey, 1.89:1); fixed. Text under a sheet's scrim is not listed (it is covered); icons are not text and are not listed; backgrounds behind glass are measured as rendered, without real refraction.
- **Keyboard:** Tab traversal on all six artboards (above).
- **Widths:** iPhone 402 pt (Pro); Mac window 1200 pt and the compact one-column card. 375, 768 and 1440 px web widths do not apply to native screens.
- **Not verified:** the SwiftUI build; SF Pro and SF Mono metrics (Inter and IBM Plex Mono stand in); real Liquid Glass; AX5 (AX3 is drawn); VoiceOver order on a device; how a real terminal pane wraps in the peek sheet.

## Self-critique

Avoid: default gradient **pass** (none). Glassmorphism everywhere **pass** (glass only on floating chrome, never on the card, the banner or host text). Emoji or icon grids **pass**. Three identical cards in a row **pass** (the three reason banners on the stuck boards are three real parallel states laid out as documentation). Centred hero with vague copy **pass** (none). Everything rounded and soft-shadowed **partial**: the radii are the platforms' concentric ones; shadows only on windows, phones and sheets. Lorem ipsum **pass**. Generic Inter on white **pass** (Inter stands in for SF Pro, the real face).

Require: at most two typefaces **pass** (SF Pro, SF Mono). Spacing scale **pass** (4 pt). One accent **pass** (rope amber on the active tab, primary buttons and Done only). Hierarchy and asymmetry **pass** (3:2 card on Mac; header, then messages, then waiting). Real content **pass**. Motion with meaning **pass**. Empty, loading, error, hover and focus states **pass** (card gallery, peek states, pressed/hover rows, focus rings).

Rules: T1 pass · T2 pass · T3 deviation · T4 deviation · T5 pass · T6 pass · T7 pass · T8 n/a · C1 pass · C2 pass · C3 pass · C4 pass · C5 pass · C6 pass · C7 n/a · L1 pass · L2 pass · L3 pass · L4 pass · L5 pass · L6 pass · L7 pass · L8 n/a · L9 pass · I1 n/a · I2 pass · I3 pass · I4 n/a · I5 n/a · I6 pass · P1 pass · P2 pass · P3 pass · P4 pass · P5 pass · P6 pass · M1 pass · M2 pass · M3 pass · M4 pass · M5 pass · K1 deviation · K2 pass · K3 pass · K4 pass · K5 pass · K6 pass · K7 pass · R1 to R7 pass (no new tokens) · A1 pass · A2 pass · A3 pass · A4 pass · A5 pass · A5b pass · A6 pass.

## Rule deviations

- **T3:** iPhone tab labels are about 11 pt (the system tab bar) and Mac captions 12 pt, as in the approved records; both pass contrast.
- **T4:** 12 and 13 (Mac), 13, 15 and 17 (iPhone) are within 1.2×: the platform text styles.
- **K1:** the radii are the concentric families of the app records, not three fixed radii.
- **L8:** native screens; 1440 and 375 px web layouts do not apply.
- **Design system vs app:** as decided by the user for both apps, materials and shapes follow iOS 26 and macOS 26; accent, voice and the mono rule are kept.

## Design record changes

No DESIGN.md exists and this package does not own one. Proposed additions when one is written: the **PM card** as a named component for both app surfaces (header with activity word and glyph, two untrusted previews, delivery line, Waiting on rows); the **stuck banner** (yellow tint inside the card, fixed reason lines, PM messages and Pause project…); the rule that **stuck notifications carry a fixed title and the project only**; the **peek sheet** (untrusted mono, Not verified pill, masked chips, dropped on close, never on a glanceable surface). The tokens are unchanged.

## Kept, moved, changed, removed

- **Kept:** every existing overview section and its order; the status card; the Messages screens (plan-149); the pause sheets; notifications for other kinds; the menu bar count; widgets and the Live Activity.
- **Moved:** nothing.
- **Changed:** the overview gains the Project manager section (iPhone after the status card, Mac at the top); the Mac sidebar project row can show "PM stuck".
- **Removed:** nothing.

## Open questions

1. **Absent vs null `pm`.** kit-pm decodes both an older host (no `pm`) and "no active PM" (`null`) as nil, so the app shows "No PM" for an older host too. Should an older host instead say "Update Capstan on build-host to see the PM"?
2. **Waiting on links.** The design opens Plans for reports, reviews, verify runs and plan reviews, because the apps have no separate review or verify list. Is that the intended list, or should verify runs open nothing until there is a list?
3. **Peek config.** Does `peek = true` take effect without restarting the host's controller? The off state says only "turn it on" and "Try again".
4. **Plan number in plan-151.** plan-151's peek-apps names `docs/design/plan-150-design-pm/peek-*.png`; the files are under `docs/design/plan-152-design-pm/`. The architect may want to correct that path.
5. **Stuck on the Mac sidebar.** The "PM stuck" word on the project row is not in the package text (which names the card and notifications). Keep it, or leave the sidebar unchanged?

## Contrast table

Every text and control element listed per PNG, in the order of `contrast.csv` (one row each, matching row for row). Foreground and background are the pixels `contrast.py` samples; the ratio is what it prints.

### card-ios-light.png (115 elements, lowest 5.17:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| card-ios-light.png | p.phcap "1 · Overview with the PM card (Worker reports…" | `#1b1f23` | `#b6bdba` | 8.66 |
| card-ios-light.png | button.gb "Hosts" | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | h1.lt "payments-api" | `#1c2024` | `#efefec` | 14.22 |
| card-ios-light.png | p.lsub "build-host · ~/src/payments-api" | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | span.ipill "Moving" | `#1d6b35` | `#e8f0ea` | 5.64 |
| card-ios-light.png | div.r1 "3 agents working · 2 wait for you" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.r2 "Live · updated 3 s ago" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | h2.sech "Project manager" | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | span.id "pm-1" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.act "Working" | `#1c5ab7` | `#ffffff` | 6.57 |
| card-ios-light.png | span.since "since 11:58 · 23 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.ml "From the PM · 12:14" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.imsg span.ml "You sent · 12:02" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" | `#1b1f23` | `#ffffff` | 16.58 |
| card-ios-light.png | button.imsg span.dl "Read by the PM · 12:03" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.isub "Waiting on" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Worker reports" | `#1c2024` | `#edeeee` | 14.10 |
| card-ios-light.png | button.ir b "3" | `#1c2024` | `#edeeee` | 14.10 |
| card-ios-light.png | button.ir span.rv "· oldest 2 h 10 min" | `#545b63` | `#edeeee` | 5.92 |
| card-ios-light.png | button.ir span "Reviews" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "1" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· 25 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Verify runs" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "1" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· 4 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "You" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "2" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· oldest 12 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.tab "Waiting" | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Agents" | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Plans" | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Messages" | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Hosts" | `#8a521c` | `#e7e8e5` | 5.17 |
| card-ios-light.png | p.phcap "2 · Largest text (AX3): rows stack; Full Keyb…" | `#1b1f23` | `#b6bdba` | 8.66 |
| card-ios-light.png | h2.sech "Project manager" (2) | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | span.id "pm-1" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.act "Working" (2) | `#1c5ab7` | `#ffffff` | 6.57 |
| card-ios-light.png | span.since "since 11:58 · 23 min" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.ml "From the PM · 12:14" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.imsg span.ml "You sent · 12:02" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.imsg span.dl "Read by the PM · 12:03" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.isub "Waiting on" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Worker reports" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "3" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· oldest 2 h 10 min" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Reviews" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "1" (3) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· 25 min" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.phcap "3 · Host offline: last known values with thei…" | `#1b1f23` | `#b6bdba` | 8.66 |
| card-ios-light.png | button.gb "Hosts" (2) | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | h1.lt "payments-api" (2) | `#1c2024` | `#efefec` | 14.22 |
| card-ios-light.png | p.lsub "build-host · ~/src/payments-api" (2) | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | b "build-host is offline" | `#1c2024` | `#e2dfd4` | 12.28 |
| card-ios-light.png | span.bt "Reconnecting in 8 s. Nothing here can change …" | `#1c2024` | `#e2dfd4` | 12.28 |
| card-ios-light.png | h2.sech "Project manager" (3) | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | span.cnt "as of 12:31" | `#545b63` | `#efefec` | 5.97 |
| card-ios-light.png | span.id "pm-1" (3) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.ipill "Last known · 12:31" | `#545b63` | `#edeeee` | 5.92 |
| card-ios-light.png | span.since "Working at 12:31 · since 11:58" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.ml "From the PM · 12:14" (3) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" (3) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.imsg span.ml "You sent · 12:29" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mt "Skip the nightly load test this week." | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.imsg span.dl "Queued at 12:31: the PM gets it when it is fr…" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.isub "Waiting on, at 12:31" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Worker reports" (3) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "3" (3) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· oldest 2 h 10 min" (3) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "Plan reviews" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "1" (4) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· 1 h 5 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.ir span "You" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir b "2" (2) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "· oldest 12 min" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.tab "Waiting" (2) | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Agents" (2) | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Plans" (2) | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Messages" (2) | `#1c2024` | `#f8f8f5` | 15.40 |
| card-ios-light.png | button.tab "Hosts" (2) | `#8a521c` | `#e7e8e5` | 5.17 |
| card-ios-light.png | p.plabel "No PM on this project" | `#1a1e23` | `#b6bdba` | 8.75 |
| card-ios-light.png | span.act "No PM" | `#535a62` | `#ffffff` | 6.98 |
| card-ios-light.png | span.r2 "No PM is active on payments-api, so nobody ro…" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span "Messages to PM" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | button.ir span.rv "kept until a PM runs" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.plabel "Stale: the host is live but the card did not …" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-ios-light.png | span.id "pm-1" (4) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.ipill "Stale · as of 12:38" | `#6b5100` | `#f1eee5` | 6.46 |
| card-ios-light.png | span.since "Idle at 12:38 · since 12:20" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.ifoot "The last refresh of this card failed 3 min ag…" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.plabel "First snapshot: skeleton shaped like the card" | `#1a1d22` | `#aeb5b2` | 8.09 |
| card-ios-light.png | span.since "Reading build-host…" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.plabel "Nothing yet: no messages, nobody waits" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-ios-light.png | span.id "pm-1" (5) | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.act "Idle" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | span.since "since 09:05 · 3 h 26 min" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | span.ml "From the PM" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | span.mm "No message to you yet." | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.ml "You sent" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | button.imsg span.mm "Nothing yet. Write to pm-1 in Messages." | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.isub "Waiting on" (3) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.ifoot "Nobody waits on anything." | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.plabel "Activity: a word and a glyph each" | `#191e22` | `#aeb5b2` | 8.04 |
| card-ios-light.png | span.act "Working" (3) | `#1c5ab6` | `#fdfdfd` | 6.48 |
| card-ios-light.png | span.act "Idle" (2) | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | span.act "Blocked" | `#a3221a` | `#ffffff` | 7.50 |
| card-ios-light.png | span.act "Done" | `#1d6b35` | `#ffffff` | 6.55 |
| card-ios-light.png | span.act "Unknown" | `#545b63` | `#ffffff` | 6.88 |
| card-ios-light.png | p.plabel "Delivery of your last message" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-ios-light.png | span.dl "Queued: the PM gets it when it is free" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.dl "Delivered to the PM · 12:03" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.dl "Read by the PM · 12:05" | `#1c2024` | `#ffffff` | 16.39 |
| card-ios-light.png | span.dl "Not delivered (expired)" | `#a3221a` | `#ffffff` | 7.50 |

### card-ios-dark.png (115 elements, lowest 5.25:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| card-ios-dark.png | p.phcap "1 · Overview with the PM card (Worker reports…" | `#e8eaec` | `#2e3238` | 10.69 |
| card-ios-dark.png | button.gb "Hosts" | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | h1.lt "payments-api" | `#eceef0` | `#101214` | 16.14 |
| card-ios-dark.png | p.lsub "build-host · ~/src/payments-api" | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | span.ipill "Moving" | `#6fd68b` | `#24332d` | 7.35 |
| card-ios-dark.png | div.r1 "3 agents working · 2 wait for you" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.r2 "Live · updated 3 s ago" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | h2.sech "Project manager" | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | span.id "pm-1" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.act "Working" | `#7fb0ff` | `#1b1d20` | 7.69 |
| card-ios-dark.png | span.since "since 11:58 · 23 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.ml "From the PM · 12:14" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.ml "You sent · 12:02" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.dl "Read by the PM · 12:03" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.isub "Waiting on" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Worker reports" | `#eceef0` | `#2f3134` | 11.22 |
| card-ios-dark.png | button.ir b "3" | `#eceef0` | `#2f3134` | 11.22 |
| card-ios-dark.png | button.ir span.rv "· oldest 2 h 10 min" | `#a6adb4` | `#2f3134` | 5.75 |
| card-ios-dark.png | button.ir span "Reviews" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "1" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· 25 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Verify runs" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "1" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· 4 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "You" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "2" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· oldest 12 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.tab "Waiting" | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Agents" | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Plans" | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Messages" | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Hosts" | `#e0a45e` | `#373a3e` | 5.25 |
| card-ios-dark.png | p.phcap "2 · Largest text (AX3): rows stack; Full Keyb…" | `#e9ebed` | `#2e3238` | 10.78 |
| card-ios-dark.png | h2.sech "Project manager" (2) | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | span.id "pm-1" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.act "Working" (2) | `#7fb0ff` | `#1b1d20` | 7.69 |
| card-ios-dark.png | span.since "since 11:58 · 23 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.ml "From the PM · 12:14" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.ml "You sent · 12:02" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.dl "Read by the PM · 12:03" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.isub "Waiting on" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Worker reports" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "3" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· oldest 2 h 10 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Reviews" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "1" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· 25 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.phcap "3 · Host offline: last known values with thei…" | `#e6e8ea` | `#2e3238` | 10.49 |
| card-ios-dark.png | button.gb "Hosts" (2) | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | h1.lt "payments-api" (2) | `#eceef0` | `#101214` | 16.14 |
| card-ios-dark.png | p.lsub "build-host · ~/src/payments-api" (2) | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | b "build-host is offline" | `#eceef0` | `#2b271c` | 12.81 |
| card-ios-dark.png | span.bt "Reconnecting in 8 s. Nothing here can change …" | `#eceef0` | `#2b271c` | 12.81 |
| card-ios-dark.png | h2.sech "Project manager" (3) | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | span.cnt "as of 12:31" | `#a6adb4` | `#101214` | 8.28 |
| card-ios-dark.png | span.id "pm-1" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.ipill "Last known · 12:31" | `#a6adb4` | `#2f3134` | 5.75 |
| card-ios-dark.png | span.since "Working at 12:31 · since 11:58" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.ml "From the PM · 12:14" (3) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Plan-15 is in review. I asked architect-3 to …" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.ml "You sent · 12:29" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mt "Skip the nightly load test this week." | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.imsg span.dl "Queued at 12:31: the PM gets it when it is fr…" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.isub "Waiting on, at 12:31" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Worker reports" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "3" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· oldest 2 h 10 min" (3) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "Plan reviews" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "1" (4) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· 1 h 5 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.ir span "You" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir b "2" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "· oldest 12 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.tab "Waiting" (2) | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Agents" (2) | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Plans" (2) | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Messages" (2) | `#eceef0` | `#24272b` | 12.90 |
| card-ios-dark.png | button.tab "Hosts" (2) | `#e0a45e` | `#373a3e` | 5.25 |
| card-ios-dark.png | p.plabel "No PM on this project" | `#eaecee` | `#2e3238` | 10.88 |
| card-ios-dark.png | span.act "No PM" | `#a5acb3` | `#1a1c1f` | 7.44 |
| card-ios-dark.png | span.r2 "No PM is active on payments-api, so nobody ro…" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span "Messages to PM" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | button.ir span.rv "kept until a PM runs" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.plabel "Stale: the host is live but the card did not …" | `#eceef0` | `#30343a` | 10.76 |
| card-ios-dark.png | span.id "pm-1" (4) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.ipill "Stale · as of 12:38" | `#f0c95a` | `#343127` | 8.18 |
| card-ios-dark.png | span.since "Idle at 12:38 · since 12:20" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.ifoot "The last refresh of this card failed 3 min ag…" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.plabel "First snapshot: skeleton shaped like the card" | `#e1e3e5` | `#2a2d32` | 10.74 |
| card-ios-dark.png | span.since "Reading build-host…" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.plabel "Nothing yet: no messages, nobody waits" | `#eceef0` | `#30343a` | 10.76 |
| card-ios-dark.png | span.id "pm-1" (5) | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.act "Idle" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | span.since "since 09:05 · 3 h 26 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | span.ml "From the PM" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | span.mm "No message to you yet." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.ml "You sent" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | button.imsg span.mm "Nothing yet. Write to pm-1 in Messages." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.isub "Waiting on" (3) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.ifoot "Nobody waits on anything." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.plabel "Activity: a word and a glyph each" | `#dee0e1` | `#2a2d32` | 10.43 |
| card-ios-dark.png | span.act "Working" (3) | `#7eaffe` | `#1a1c1f` | 7.68 |
| card-ios-dark.png | span.act "Idle" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | span.act "Blocked" | `#ffa49c` | `#1b1d20` | 8.88 |
| card-ios-dark.png | span.act "Done" | `#6fd68b` | `#1b1d20` | 9.38 |
| card-ios-dark.png | span.act "Unknown" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-ios-dark.png | p.plabel "Delivery of your last message" | `#eceef0` | `#30343a` | 10.76 |
| card-ios-dark.png | span.dl "Queued: the PM gets it when it is free" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.dl "Delivered to the PM · 12:03" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.dl "Read by the PM · 12:05" | `#eceef0` | `#1b1d20` | 14.52 |
| card-ios-dark.png | span.dl "Not delivered (expired)" | `#ffa49c` | `#1b1d20` | 8.88 |

### card-mac-light.png (106 elements, lowest 5.05:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| card-mac-light.png | a "Waiting for you" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a span.badge "7" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | a "Agents" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a span.badge "12" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | a "Plans" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a span.badge "4" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | a "Messages to PM" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a span.badge "3" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | div.sh "build-host" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | a "payments-api" | `#1c2024` | `#ebe4dc` | 13.00 |
| card-mac-light.png | a "docs-site" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a "mobile-app" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | a span.sub "paused" | `#545b63` | `#fafaf8` | 6.58 |
| card-mac-light.png | span.title "payments-api" | `#1c2024` | `#fafaf8` | 15.68 |
| card-mac-light.png | h1.h2 "payments-api" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.path "build-host · ~/src/payments-api" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.pill "Moving" | `#1d6b35` | `#e0e8e0` | 5.24 |
| card-mac-light.png | div.ovstat "3 agents working · 2 wait for you" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.muted "Live · updated 3 s ago" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | h2.grp "Project manager" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.id "pm-1" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | span.act "Working" | `#1d5bb8` | `#ededeb` | 5.53 |
| card-mac-light.png | span.since "since 11:58 · 23 min" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | p.lbl "From the PM · 12:14" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.mrow span.mt "Plan-15 is in review. I asked architect-3 to …" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.mrow span.go "Open conversation ›" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | p.lbl "You sent · 12:02" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.mrow span.mt "Please hold plan-15 until finance signs off t…" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.mrow span.dl "Read by the PM · 12:03" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.mrow span.go "Open conversation ›" (2) | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | p.lbl "Waiting on" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.wrow span "Worker reports" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow b "3" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow span.wc "· oldest 2 h 10 min" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.wrow span "Reviews" | `#1c2024` | `#dcdddb` | 12.02 |
| card-mac-light.png | button.wrow b "1" | `#1c2024` | `#dcdddb` | 12.02 |
| card-mac-light.png | button.wrow span.wc "· 25 min" | `#545b63` | `#dcdddb` | 5.05 |
| card-mac-light.png | button.wrow span "Verify runs" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow b "1" (2) | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow span.wc "· 4 min" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.wrow span "Plan reviews" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow b "1" (3) | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow span.wc "· 1 h 5 min" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | button.wrow span "You" | `#1d2125` | `#ededeb` | 13.82 |
| card-mac-light.png | button.wrow b "2" | `#1c2024` | `#ededeb` | 13.98 |
| card-mac-light.png | button.wrow span.wc "· oldest 12 min" | `#545b63` | `#ededeb` | 5.87 |
| card-mac-light.png | h2.grp "Waiting for you · 2" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.k "Command proposal · dev-3" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.t "12 min" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | code "npm run db:migrate -- --env staging" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.k "Design approval · designer-2" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.t "3 min" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.m "Refund screens, light and dark" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.plabel "No PM on this project" | `#1a1e23` | `#aeb5b2` | 8.02 |
| card-mac-light.png | span.act "No PM" | `#535a62` | `#f5f5f3` | 6.40 |
| card-mac-light.png | p.note "No PM is active on payments-api, so nobody ro…" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | p.plabel "Host offline: last known values" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-mac-light.png | span.id "pm-1" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.pill "Last known · 12:31" | `#545b63` | `#e4e5e3` | 5.44 |
| card-mac-light.png | span.since "Working at 12:31" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.note "build-host is offline; reconnecting in 8 s. T…" | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | p.plabel "Stale: the card did not refresh" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-mac-light.png | span.id "pm-1" (3) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.pill "Stale · as of 12:38" | `#6b5100` | `#e9e6db` | 6.00 |
| card-mac-light.png | button.btn "Refresh" | `#1c2024` | `#e4e5e3` | 12.97 |
| card-mac-light.png | p.note "The last refresh failed 3 min ago." | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.kbd "⌘R" | `#1c2024` | `#e4e5e3` | 12.97 |
| card-mac-light.png | p.plabel "First snapshot: skeleton shaped like the card" | `#1a1e23` | `#aeb5b2` | 8.02 |
| card-mac-light.png | span.since "Reading build-host…" | `#535a62` | `#f5f5f3` | 6.40 |
| card-mac-light.png | p.plabel "Nothing yet" | `#1c2024` | `#b9c0bd` | 8.85 |
| card-mac-light.png | span.id "pm-1" (4) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | span.act "Idle" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.since "since 09:05 · 3 h 26 min" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.lbl "From the PM" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.none "No message to you yet." | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.lbl "You sent" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.none "Nothing yet." | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.kbd "⇧⌘N" | `#1c2024` | `#e4e5e3` | 12.97 |
| card-mac-light.png | p.lbl "Waiting on" (2) | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.none "Nobody waits on anything." | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | p.plabel "Words: activity and delivery" | `#1b1f23` | `#b6bdba` | 8.66 |
| card-mac-light.png | span.act "Working" (2) | `#1d5bb8` | `#f6f6f4` | 5.99 |
| card-mac-light.png | span.act "Idle" (2) | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.act "Blocked" | `#a3221a` | `#f6f6f4` | 6.93 |
| card-mac-light.png | span.act "Done" | `#1d6b35` | `#f6f6f4` | 6.05 |
| card-mac-light.png | span.act "Unknown" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.dl "Queued: the PM gets it when it is free" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.dl "Delivered to the PM · 12:03" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.dl "Read by the PM · 12:05" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | span.dl "Not delivered (expired)" | `#a3221a` | `#f6f6f4` | 6.93 |
| card-mac-light.png | p.plabel "Compact window (overview under 640 px): one c…" | `#1a1e23` | `#aeb5b2` | 8.02 |
| card-mac-light.png | span.id "pm-1" (5) | `#1b1f23` | `#f5f5f3` | 15.19 |
| card-mac-light.png | span.act "Blocked" (2) | `#a12119` | `#f5f5f3` | 7.00 |
| card-mac-light.png | span.since "since 12:20 · 11 min" | `#535a62` | `#f5f5f3` | 6.40 |
| card-mac-light.png | p.lbl "From the PM · 12:14" (2) | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | button.mrow span.mt "Plan-15 is in review. I asked architect-3 to …" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | p.lbl "You sent · 12:29" | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | button.mrow span.mt "Use the new seed data too." | `#1d2125` | `#f6f6f4` | 14.97 |
| card-mac-light.png | button.mrow span.dl "Not delivered (cancelled)" | `#a3221a` | `#f6f6f4` | 6.93 |
| card-mac-light.png | p.lbl "Waiting on" (3) | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | button.wrow span "Worker reports" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | button.wrow b "3" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | button.wrow span.wc "· oldest 2 h 10 min" (2) | `#545b63` | `#f6f6f4` | 6.36 |
| card-mac-light.png | button.wrow span "You" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | button.wrow b "2" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| card-mac-light.png | button.wrow span.wc "· oldest 12 min" (2) | `#545b63` | `#f6f6f4` | 6.36 |

### card-mac-dark.png (106 elements, lowest 5.18:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| card-mac-dark.png | a "Waiting for you" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a span.badge "7" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | a "Agents" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a span.badge "12" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | a "Plans" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a span.badge "4" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | a "Messages to PM" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a span.badge "3" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | div.sh "build-host" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | a "payments-api" | `#eceef0` | `#453d37` | 9.14 |
| card-mac-dark.png | a "docs-site" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a "mobile-app" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | a span.sub "paused" | `#a6adb4` | `#272b30` | 6.28 |
| card-mac-dark.png | span.title "payments-api" | `#eceef0` | `#272b30` | 12.24 |
| card-mac-dark.png | h1.h2 "payments-api" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.path "build-host · ~/src/payments-api" | `#a5acb3` | `#1b1d20` | 7.36 |
| card-mac-dark.png | span.pill "Moving" | `#6fd68b` | `#24332d` | 7.35 |
| card-mac-dark.png | div.ovstat "3 agents working · 2 wait for you" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.muted "Live · updated 3 s ago" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | h2.grp "Project manager" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.id "pm-1" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | span.act "Working" | `#7fb0ff` | `#232528` | 6.99 |
| card-mac-dark.png | span.since "since 11:58 · 23 min" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | p.lbl "From the PM · 12:14" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.mrow span.mt "Plan-15 is in review. I asked architect-3 to …" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.mrow span.go "Open conversation ›" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | p.lbl "You sent · 12:02" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.mrow span.mt "Please hold plan-15 until finance signs off t…" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.mrow span.dl "Read by the PM · 12:03" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.mrow span.go "Open conversation ›" (2) | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | p.lbl "Waiting on" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.wrow span "Worker reports" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow b "3" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow span.wc "· oldest 2 h 10 min" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.wrow span "Reviews" | `#eceef0` | `#36383b` | 10.11 |
| card-mac-dark.png | button.wrow b "1" | `#eceef0` | `#36383b` | 10.11 |
| card-mac-dark.png | button.wrow span.wc "· 25 min" | `#a6adb4` | `#36383b` | 5.18 |
| card-mac-dark.png | button.wrow span "Verify runs" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow b "1" (2) | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow span.wc "· 4 min" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.wrow span "Plan reviews" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow b "1" (3) | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow span.wc "· 1 h 5 min" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | button.wrow span "You" | `#ebedef` | `#232528` | 13.09 |
| card-mac-dark.png | button.wrow b "2" | `#eceef0` | `#232528` | 13.21 |
| card-mac-dark.png | button.wrow span.wc "· oldest 12 min" | `#a6adb4` | `#232528` | 6.77 |
| card-mac-dark.png | h2.grp "Waiting for you · 2" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.k "Command proposal · dev-3" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.t "12 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | code "npm run db:migrate -- --env staging" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.k "Design approval · designer-2" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.t "3 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.m "Refund screens, light and dark" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.plabel "No PM on this project" | `#dddfe1` | `#272a2f` | 10.78 |
| card-mac-dark.png | span.act "No PM" | `#a4abb2` | `#1a1c1f` | 7.36 |
| card-mac-dark.png | p.note "No PM is active on payments-api, so nobody ro…" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | p.plabel "Host offline: last known values" | `#eceef0` | `#30343a` | 10.76 |
| card-mac-dark.png | span.id "pm-1" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.pill "Last known · 12:31" | `#a6adb4` | `#2f3134` | 5.75 |
| card-mac-dark.png | span.since "Working at 12:31" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.note "build-host is offline; reconnecting in 8 s. T…" | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | p.plabel "Stale: the card did not refresh" | `#eceef0` | `#30343a` | 10.76 |
| card-mac-dark.png | span.id "pm-1" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.pill "Stale · as of 12:38" | `#f0c95a` | `#343127` | 8.18 |
| card-mac-dark.png | button.btn "Refresh" | `#eceef0` | `#2f3134` | 11.22 |
| card-mac-dark.png | p.note "The last refresh failed 3 min ago." | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.kbd "⌘R" | `#eceef0` | `#2f3134` | 11.22 |
| card-mac-dark.png | p.plabel "First snapshot: skeleton shaped like the card" | `#ced0d2` | `#272a2f` | 9.31 |
| card-mac-dark.png | span.since "Reading build-host…" | `#a4abb2` | `#1a1c1f` | 7.36 |
| card-mac-dark.png | p.plabel "Nothing yet" | `#eceef0` | `#30343a` | 10.76 |
| card-mac-dark.png | span.id "pm-1" (4) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | span.act "Idle" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.since "since 09:05 · 3 h 26 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.lbl "From the PM" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.none "No message to you yet." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.lbl "You sent" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.none "Nothing yet." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.kbd "⇧⌘N" | `#eceef0` | `#2f3134` | 11.22 |
| card-mac-dark.png | p.lbl "Waiting on" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.none "Nobody waits on anything." | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | p.plabel "Words: activity and delivery" | `#e9ebed` | `#2e3238` | 10.78 |
| card-mac-dark.png | span.act "Working" (2) | `#7fb0ff` | `#1b1d20` | 7.69 |
| card-mac-dark.png | span.act "Idle" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.act "Blocked" | `#ffa49c` | `#1b1d20` | 8.88 |
| card-mac-dark.png | span.act "Done" | `#6fd68b` | `#1b1d20` | 9.38 |
| card-mac-dark.png | span.act "Unknown" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.dl "Queued: the PM gets it when it is free" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.dl "Delivered to the PM · 12:03" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.dl "Read by the PM · 12:05" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | span.dl "Not delivered (expired)" | `#ffa49c` | `#1b1d20` | 8.88 |
| card-mac-dark.png | p.plabel "Compact window (overview under 640 px): one c…" | `#ced0d2` | `#272a2f` | 9.31 |
| card-mac-dark.png | span.id "pm-1" (5) | `#eaecee` | `#1a1c1f` | 14.42 |
| card-mac-dark.png | span.act "Blocked" (2) | `#fda29a` | `#1a1c1f` | 8.79 |
| card-mac-dark.png | span.since "since 12:20 · 11 min" | `#a4abb2` | `#1a1c1f` | 7.36 |
| card-mac-dark.png | p.lbl "From the PM · 12:14" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | button.mrow span.mt "Plan-15 is in review. I asked architect-3 to …" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | p.lbl "You sent · 12:29" | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | button.mrow span.mt "Use the new seed data too." | `#ebedef` | `#1b1d20` | 14.39 |
| card-mac-dark.png | button.mrow span.dl "Not delivered (cancelled)" | `#ffa49c` | `#1b1d20` | 8.88 |
| card-mac-dark.png | p.lbl "Waiting on" (3) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | button.wrow span "Worker reports" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | button.wrow b "3" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | button.wrow span.wc "· oldest 2 h 10 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| card-mac-dark.png | button.wrow span "You" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | button.wrow b "2" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| card-mac-dark.png | button.wrow span.wc "· oldest 12 min" (2) | `#a6adb4` | `#1b1d20` | 7.45 |

### stuck-ios-light.png (61 elements, lowest 5.17:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| stuck-ios-light.png | p.phcap "1 · Unread mail: the banner opens the PM card" | `#1b1f23` | `#b6bdba` | 8.66 |
| stuck-ios-light.png | button.gb "Hosts" | `#1c2024` | `#f8f8f5` | 15.40 |
| stuck-ios-light.png | h1.lt "payments-api" | `#1c2024` | `#efefec` | 14.22 |
| stuck-ios-light.png | p.lsub "build-host · ~/src/payments-api" | `#545b63` | `#efefec` | 5.97 |
| stuck-ios-light.png | h2.sech "Project manager" | `#545b63` | `#efefec` | 5.97 |
| stuck-ios-light.png | b "pm-1 looks stuck" | `#1c2024` | `#f1eee5` | 14.13 |
| stuck-ios-light.png | span.bt "It is idle while your message has waited unre…" | `#1c2024` | `#f1eee5` | 14.13 |
| stuck-ios-light.png | button.ibtn "PM messages" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Pause project…" | `#a3221a` | `#ffffff` | 7.50 |
| stuck-ios-light.png | span.id "pm-1" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | span.act "Idle" | `#535a62` | `#ffffff` | 6.98 |
| stuck-ios-light.png | span.since "since 12:12 · 19 min" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | button.imsg span.ml "You sent · 12:14" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" | `#1b1f23` | `#ffffff` | 16.58 |
| stuck-ios-light.png | button.imsg span.dl "Queued: the PM gets it when it is free" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | p.isub "Waiting on" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | button.ir span "Worker reports" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ir b "3" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ir span.rv "· oldest 2 h 10 min" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | button.ir span "You" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ir b "1" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ir span.rv "· 6 min" | `#545b63` | `#ffffff` | 6.88 |
| stuck-ios-light.png | button.tab "Waiting" | `#1c2024` | `#f8f8f5` | 15.40 |
| stuck-ios-light.png | button.tab "Agents" | `#1c2024` | `#f8f8f5` | 15.40 |
| stuck-ios-light.png | button.tab "Plans" | `#1c2024` | `#f8f8f5` | 15.40 |
| stuck-ios-light.png | button.tab "Messages" | `#1c2024` | `#f8f8f5` | 15.40 |
| stuck-ios-light.png | button.tab "Hosts" | `#8a521c` | `#e7e8e5` | 5.17 |
| stuck-ios-light.png | p.phcap "2 · Local notification: title and project, no…" | `#1b1f23` | `#b6bdba` | 8.66 |
| stuck-ios-light.png | span.ldate "Thursday 5 November" | `#1c2024` | `#b9c0bd` | 8.85 |
| stuck-ios-light.png | span.ltime "12:31" | `#1c2024` | `#b9c0bd` | 8.85 |
| stuck-ios-light.png | span.nt "PM looks stuck" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-ios-light.png | span.nw "now" | `#1b1f23` | `#e4e8e5` | 13.40 |
| stuck-ios-light.png | span.ns "payments-api" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-ios-light.png | span.nt "Command proposal" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-ios-light.png | span.nw "9 min ago" | `#1b1f23` | `#e4e8e5` | 13.40 |
| stuck-ios-light.png | span.ns "payments-api" (2) | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-ios-light.png | p.phcap "3 · Pause project… (the existing sheet; Cance…" | `#1b1f23` | `#b6bdba` | 8.66 |
| stuck-ios-light.png | h2.h2 "Pause payments-api?" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | p "Every agent of payments-api stops after its c…" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | code "cstan resume" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | label "Reason (optional)" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Cancel" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Pause project" | `#ffffff` | `#a3221a` | 7.50 |
| stuck-ios-light.png | p.plabel "No progress" | `#1b1e23` | `#b6bdba` | 8.74 |
| stuck-ios-light.png | b "pm-1 looks stuck" (2) | `#1b1f23` | `#f1eee5` | 14.29 |
| stuck-ios-light.png | span.bt "It shows as working but has made no progress …" | `#1c2024` | `#f1eee5` | 14.13 |
| stuck-ios-light.png | button.ibtn "PM messages" (2) | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Pause project…" (2) | `#a3221a` | `#ffffff` | 7.50 |
| stuck-ios-light.png | p.plabel "Stopped at a prompt" | `#1a1e21` | `#aeb5b2` | 8.03 |
| stuck-ios-light.png | b "pm-1 looks stuck" (3) | `#1b1f23` | `#f1eee5` | 14.29 |
| stuck-ios-light.png | span.bt "It has been stopped at a prompt for 15 min, s…" | `#1c2024` | `#f1eee5` | 14.13 |
| stuck-ios-light.png | button.ibtn "PM messages" (3) | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Pause project…" (3) | `#a3221a` | `#ffffff` | 7.50 |
| stuck-ios-light.png | span.tag151 "plan-151 · not built in plan-152" | `#1a1e23` | `#e0e0df` | 12.68 |
| stuck-ios-light.png | p.plabel "With Peek" | `#1a1e23` | `#a9b0ad` | 7.58 |
| stuck-ios-light.png | b "pm-1 looks stuck" (4) | `#1b1f23` | `#f1eee5` | 14.29 |
| stuck-ios-light.png | span.bt "It shows as working but has made no progress …" (2) | `#1c2024` | `#f1eee5` | 14.13 |
| stuck-ios-light.png | button.ibtn "Peek" | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "PM messages" (4) | `#1c2024` | `#ffffff` | 16.39 |
| stuck-ios-light.png | button.ibtn "Pause project…" (4) | `#a3221a` | `#ffffff` | 7.50 |
| stuck-ios-light.png | input "pm-1 looks stuck" | `#1c2024` | `#f6f6f6` | 15.16 |

### stuck-ios-dark.png (61 elements, lowest 5.25:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| stuck-ios-dark.png | p.phcap "1 · Unread mail: the banner opens the PM card" | `#e6e8ea` | `#2e3238` | 10.49 |
| stuck-ios-dark.png | button.gb "Hosts" | `#eceef0` | `#23262b` | 13.05 |
| stuck-ios-dark.png | h1.lt "payments-api" | `#eceef0` | `#101214` | 16.14 |
| stuck-ios-dark.png | p.lsub "build-host · ~/src/payments-api" | `#a6adb4` | `#101214` | 8.28 |
| stuck-ios-dark.png | h2.sech "Project manager" | `#a6adb4` | `#101214` | 8.28 |
| stuck-ios-dark.png | b "pm-1 looks stuck" | `#eceef0` | `#343127` | 11.18 |
| stuck-ios-dark.png | span.bt "It is idle while your message has waited unre…" | `#eceef0` | `#343127` | 11.18 |
| stuck-ios-dark.png | button.ibtn "PM messages" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Pause project…" | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-ios-dark.png | span.id "pm-1" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | span.act "Idle" | `#a6adb4` | `#1a1c1f` | 7.53 |
| stuck-ios-dark.png | span.since "since 12:12 · 19 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | button.imsg span.ml "You sent · 12:14" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | button.imsg span.mt "Please hold plan-15 until finance signs off t…" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.imsg span.dl "Queued: the PM gets it when it is free" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | p.isub "Waiting on" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | button.ir span "Worker reports" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ir b "3" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ir span.rv "· oldest 2 h 10 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | button.ir span "You" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ir b "1" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ir span.rv "· 6 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-ios-dark.png | button.tab "Waiting" | `#eceef0` | `#23262b` | 13.05 |
| stuck-ios-dark.png | button.tab "Agents" | `#eceef0` | `#23262b` | 13.05 |
| stuck-ios-dark.png | button.tab "Plans" | `#eceef0` | `#23262b` | 13.05 |
| stuck-ios-dark.png | button.tab "Messages" | `#eceef0` | `#23262b` | 13.05 |
| stuck-ios-dark.png | button.tab "Hosts" | `#e0a45e` | `#373a3d` | 5.25 |
| stuck-ios-dark.png | p.phcap "2 · Local notification: title and project, no…" | `#e6e8ea` | `#2e3238` | 10.49 |
| stuck-ios-dark.png | span.ldate "Thursday 5 November" | `#eceef0` | `#30343a` | 10.76 |
| stuck-ios-dark.png | span.ltime "12:31" | `#eceef0` | `#30343a` | 10.76 |
| stuck-ios-dark.png | span.nt "PM looks stuck" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-ios-dark.png | span.nw "now" | `#eceef0` | `#2e3239` | 11.07 |
| stuck-ios-dark.png | span.ns "payments-api" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-ios-dark.png | span.nt "Command proposal" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-ios-dark.png | span.nw "9 min ago" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-ios-dark.png | span.ns "payments-api" (2) | `#eceef0` | `#2f333a` | 10.91 |
| stuck-ios-dark.png | p.phcap "3 · Pause project… (the existing sheet; Cance…" | `#e9ebed` | `#2e3238` | 10.78 |
| stuck-ios-dark.png | h2.h2 "Pause payments-api?" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | p "Every agent of payments-api stops after its c…" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | code "cstan resume" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | label "Reason (optional)" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Cancel" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Pause project" | `#2a0805` | `#ffa49c` | 9.73 |
| stuck-ios-dark.png | p.plabel "No progress" | `#eaecee` | `#2f3339` | 10.72 |
| stuck-ios-dark.png | b "pm-1 looks stuck" (2) | `#eceef0` | `#333026` | 11.34 |
| stuck-ios-dark.png | span.bt "It shows as working but has made no progress …" | `#eceef0` | `#343127` | 11.18 |
| stuck-ios-dark.png | button.ibtn "PM messages" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Pause project…" (2) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-ios-dark.png | p.plabel "Stopped at a prompt" | `#e1e3e5` | `#272a2f` | 11.19 |
| stuck-ios-dark.png | b "pm-1 looks stuck" (3) | `#eceef0` | `#333026` | 11.34 |
| stuck-ios-dark.png | span.bt "It has been stopped at a prompt for 15 min, s…" | `#eceef0` | `#343127` | 11.18 |
| stuck-ios-dark.png | button.ibtn "PM messages" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Pause project…" (3) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-ios-dark.png | span.tag151 "plan-151 · not built in plan-152" | `#dfe0e2` | `#17191c` | 13.33 |
| stuck-ios-dark.png | p.plabel "With Peek" | `#cfd1d2` | `#272a2f` | 9.40 |
| stuck-ios-dark.png | b "pm-1 looks stuck" (4) | `#eceef0` | `#333026` | 11.34 |
| stuck-ios-dark.png | span.bt "It shows as working but has made no progress …" (2) | `#eceef0` | `#343127` | 11.18 |
| stuck-ios-dark.png | button.ibtn "Peek" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "PM messages" (4) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-ios-dark.png | button.ibtn "Pause project…" (4) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-ios-dark.png | input "pm-1 looks stuck" | `#eceef0` | `#232528` | 13.21 |

### stuck-mac-light.png (72 elements, lowest 5.24:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| stuck-mac-light.png | a "Waiting for you" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a span.badge "7" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | a "Agents" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a span.badge "12" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | a "Plans" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a span.badge "4" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | a "Messages to PM" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a span.badge "3" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | div.sh "build-host" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | a "payments-api" | `#1c2024` | `#ebe4dc` | 13.00 |
| stuck-mac-light.png | a span.sub "PM stuck" | `#545b63` | `#ebe4dc` | 5.46 |
| stuck-mac-light.png | a "docs-site" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a "mobile-app" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | a span.sub "paused" | `#545b63` | `#fafaf8` | 6.58 |
| stuck-mac-light.png | span.title "payments-api" | `#1c2024` | `#fafaf8` | 15.68 |
| stuck-mac-light.png | h1.h2 "payments-api" | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | span.path "build-host · ~/src/payments-api" | `#545b63` | `#f6f6f4` | 6.36 |
| stuck-mac-light.png | span.pill "Moving" | `#1d6b35` | `#e0e8e0` | 5.24 |
| stuck-mac-light.png | div.ovstat "3 agents working · 2 wait for you" | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | span.muted "Live · updated 3 s ago" | `#545b63` | `#f6f6f4` | 6.36 |
| stuck-mac-light.png | h2.grp "Project manager" | `#545b63` | `#f6f6f4` | 6.36 |
| stuck-mac-light.png | b "pm-1 looks stuck" | `#1c2024` | `#e0ddd3` | 12.06 |
| stuck-mac-light.png | span.bt "It shows as working but has made no progress …" | `#1c2024` | `#e0ddd3` | 12.06 |
| stuck-mac-light.png | button.btn "PM messages" | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | button.btn "Pause project…" | `#a3221a` | `#f6f6f4` | 6.93 |
| stuck-mac-light.png | span.id "pm-1" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | span.act "Working" | `#1d5bb8` | `#ededeb` | 5.53 |
| stuck-mac-light.png | span.since "since 11:58 · 33 min" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | p.lbl "From the PM · 12:05" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | button.mrow span.mt "Starting the integration of plan-14 now." | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | p.lbl "You sent · 12:02" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | button.mrow span.mt "Please hold plan-15 until finance signs off t…" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.mrow span.dl "Read by the PM · 12:03" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | p.lbl "Waiting on" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | button.wrow span "Worker reports" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.wrow b "3" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.wrow span.wc "· oldest 2 h 10 min" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | button.wrow span "Verify runs" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.wrow b "1" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.wrow span.wc "· 31 min" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | button.wrow span "You" | `#1d2125` | `#ededeb` | 13.82 |
| stuck-mac-light.png | button.wrow b "2" | `#1c2024` | `#ededeb` | 13.98 |
| stuck-mac-light.png | button.wrow span.wc "· oldest 12 min" | `#545b63` | `#ededeb` | 5.87 |
| stuck-mac-light.png | p.plabel "Notification: a title and the project, no body" | `#1a1e23` | `#aeb5b2` | 8.02 |
| stuck-mac-light.png | span.nt "PM looks stuck" | `#1c2024` | `#e3e7e4` | 13.12 |
| stuck-mac-light.png | span.nw "now" | `#545b63` | `#e3e7e4` | 5.51 |
| stuck-mac-light.png | span.ns "payments-api" | `#545b63` | `#e3e8e5` | 5.55 |
| stuck-mac-light.png | p.plabel "Menu bar: the existing count only" | `#1c2024` | `#b9c0bd` | 8.85 |
| stuck-mac-light.png | span "Finder" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-mac-light.png | span.mbx "7" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-mac-light.png | span "Thu 12:31" | `#1c2024` | `#e4e8e5` | 13.24 |
| stuck-mac-light.png | p.plabel "Unread mail" | `#1a1e23` | `#aeb5b2` | 8.02 |
| stuck-mac-light.png | b "pm-1 looks stuck" (2) | `#1b1f23` | `#e8e5da` | 13.14 |
| stuck-mac-light.png | span.bt "It is idle while your message has waited unre…" | `#1c2024` | `#e9e6db` | 13.12 |
| stuck-mac-light.png | button.btn "PM messages" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | button.btn "Pause project…" (2) | `#a3221a` | `#f6f6f4` | 6.93 |
| stuck-mac-light.png | p.plabel "Stopped at a prompt" | `#1c2024` | `#b9c0bd` | 8.85 |
| stuck-mac-light.png | b "pm-1 looks stuck" (3) | `#1c2024` | `#e9e6db` | 13.12 |
| stuck-mac-light.png | span.bt "It has been stopped at a prompt for 15 min, s…" | `#1c2024` | `#e9e6db` | 13.12 |
| stuck-mac-light.png | button.btn "PM messages" (3) | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | button.btn "Pause project…" (3) | `#a3221a` | `#f6f6f4` | 6.93 |
| stuck-mac-light.png | span.tag151 "plan-151 · not built in plan-152" | `#1a1e23` | `#e0e0df` | 12.68 |
| stuck-mac-light.png | p.plabel "With Peek" | `#1a1e23` | `#a9b0ad` | 7.58 |
| stuck-mac-light.png | b "pm-1 looks stuck" (4) | `#1b1f23` | `#e8e5da` | 13.14 |
| stuck-mac-light.png | span.bt "It shows as working but has made no progress …" (2) | `#1c2024` | `#e9e6db` | 13.12 |
| stuck-mac-light.png | button.btn "Peek" | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | button.btn "PM messages" (4) | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | button.btn "Pause project…" (4) | `#a3221a` | `#f6f6f4` | 6.93 |
| stuck-mac-light.png | p.plabel "Recovered: the banner goes; nothing else is s…" | `#1c2024` | `#b9c0bd` | 8.85 |
| stuck-mac-light.png | span.id "pm-1" (2) | `#1c2024` | `#f6f6f4` | 15.14 |
| stuck-mac-light.png | span.act "Working" (2) | `#1d5bb8` | `#f6f6f4` | 5.99 |
| stuck-mac-light.png | span.since "since 12:33 · 1 min" | `#545b63` | `#f6f6f4` | 6.36 |

### stuck-mac-dark.png (72 elements, lowest 4.69:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| stuck-mac-dark.png | a "Waiting for you" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a span.badge "7" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | a "Agents" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a span.badge "12" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | a "Plans" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a span.badge "4" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | a "Messages to PM" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a span.badge "3" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | div.sh "build-host" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | a "payments-api" | `#eceef0` | `#453d37` | 9.14 |
| stuck-mac-dark.png | a span.sub "PM stuck" | `#a6adb4` | `#453d37` | 4.69 |
| stuck-mac-dark.png | a "docs-site" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a "mobile-app" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | a span.sub "paused" | `#a6adb4` | `#272b30` | 6.28 |
| stuck-mac-dark.png | span.title "payments-api" | `#eceef0` | `#272b30` | 12.24 |
| stuck-mac-dark.png | h1.h2 "payments-api" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | span.path "build-host · ~/src/payments-api" | `#a5acb3` | `#1b1d20` | 7.36 |
| stuck-mac-dark.png | span.pill "Moving" | `#6fd68b` | `#24332d` | 7.35 |
| stuck-mac-dark.png | div.ovstat "3 agents working · 2 wait for you" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | span.muted "Live · updated 3 s ago" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-mac-dark.png | h2.grp "Project manager" | `#a6adb4` | `#1b1d20` | 7.45 |
| stuck-mac-dark.png | b "pm-1 looks stuck" | `#eceef0` | `#3b382e` | 10.08 |
| stuck-mac-dark.png | span.bt "It shows as working but has made no progress …" | `#eceef0` | `#3b382e` | 10.08 |
| stuck-mac-dark.png | button.btn "PM messages" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | button.btn "Pause project…" | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-mac-dark.png | span.id "pm-1" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | span.act "Working" | `#7fb0ff` | `#232528` | 6.99 |
| stuck-mac-dark.png | span.since "since 11:58 · 33 min" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | p.lbl "From the PM · 12:05" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | button.mrow span.mt "Starting the integration of plan-14 now." | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | p.lbl "You sent · 12:02" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | button.mrow span.mt "Please hold plan-15 until finance signs off t…" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.mrow span.dl "Read by the PM · 12:03" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | p.lbl "Waiting on" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | button.wrow span "Worker reports" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.wrow b "3" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.wrow span.wc "· oldest 2 h 10 min" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | button.wrow span "Verify runs" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.wrow b "1" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.wrow span.wc "· 31 min" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | button.wrow span "You" | `#ebedef` | `#232528` | 13.09 |
| stuck-mac-dark.png | button.wrow b "2" | `#eceef0` | `#232528` | 13.21 |
| stuck-mac-dark.png | button.wrow span.wc "· oldest 12 min" | `#a6adb4` | `#232528` | 6.77 |
| stuck-mac-dark.png | p.plabel "Notification: a title and the project, no body" | `#dddfe1` | `#2a2d32` | 10.34 |
| stuck-mac-dark.png | span.nt "PM looks stuck" | `#eceef0` | `#2e333a` | 10.94 |
| stuck-mac-dark.png | span.nw "now" | `#a6adb4` | `#2e333a` | 5.61 |
| stuck-mac-dark.png | span.ns "payments-api" | `#a6adb4` | `#2e333a` | 5.61 |
| stuck-mac-dark.png | p.plabel "Menu bar: the existing count only" | `#eceef0` | `#30343a` | 10.76 |
| stuck-mac-dark.png | span "Finder" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-mac-dark.png | span.mbx "7" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-mac-dark.png | span "Thu 12:31" | `#eceef0` | `#2f333a` | 10.91 |
| stuck-mac-dark.png | p.plabel "Unread mail" | `#caccce` | `#2a2d32` | 8.58 |
| stuck-mac-dark.png | b "pm-1 looks stuck" (2) | `#eaecee` | `#333026` | 11.14 |
| stuck-mac-dark.png | span.bt "It is idle while your message has waited unre…" | `#eceef0` | `#343127` | 11.18 |
| stuck-mac-dark.png | button.btn "PM messages" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | button.btn "Pause project…" (2) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-mac-dark.png | p.plabel "Stopped at a prompt" | `#eceef0` | `#30343a` | 10.76 |
| stuck-mac-dark.png | b "pm-1 looks stuck" (3) | `#eceef0` | `#343127` | 11.18 |
| stuck-mac-dark.png | span.bt "It has been stopped at a prompt for 15 min, s…" | `#eceef0` | `#343127` | 11.18 |
| stuck-mac-dark.png | button.btn "PM messages" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | button.btn "Pause project…" (3) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-mac-dark.png | span.tag151 "plan-151 · not built in plan-152" | `#d3d4d6` | `#151719` | 12.12 |
| stuck-mac-dark.png | p.plabel "With Peek" | `#cfd1d2` | `#272a2f` | 9.40 |
| stuck-mac-dark.png | b "pm-1 looks stuck" (4) | `#ebedef` | `#333026` | 11.24 |
| stuck-mac-dark.png | span.bt "It shows as working but has made no progress …" (2) | `#eceef0` | `#343127` | 11.18 |
| stuck-mac-dark.png | button.btn "Peek" | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | button.btn "PM messages" (4) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | button.btn "Pause project…" (4) | `#ffa49c` | `#1b1d20` | 8.88 |
| stuck-mac-dark.png | p.plabel "Recovered: the banner goes; nothing else is s…" | `#eceef0` | `#30343a` | 10.76 |
| stuck-mac-dark.png | span.id "pm-1" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| stuck-mac-dark.png | span.act "Working" (2) | `#7fb0ff` | `#1b1d20` | 7.69 |
| stuck-mac-dark.png | span.since "since 12:33 · 1 min" | `#a6adb4` | `#1b1d20` | 7.45 |

### peek-ios-light.png (58 elements, lowest 6.31:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| peek-ios-light.png | p.phcap "1 · Peek sheet (large detent): the last 40 li…" | `#1b1f23` | `#b6bdba` | 8.66 |
| peek-ios-light.png | h2.navt "pm-1 screen" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | button.gb "Done" | `#8a521c` | `#ffffff` | 6.36 |
| peek-ios-light.png | span.nv "Not verified" | `#6b5100` | `#f1eee5` | 6.46 |
| peek-ios-light.png | span "Captured" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | b "12:31:07" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | span "3 values masked" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" | `#545b63` | `#f5f5f6` | 6.31 |
| peek-ios-light.png | pre "● Bash(cstan inbox) 1 message waits for you (…" | `#1c2024` | `#f5f5f6` | 15.04 |
| peek-ios-light.png | span.mask "[masked]" | `#1c2024` | `#e3e4e5` | 12.87 |
| peek-ios-light.png | span.mask "[masked]" (2) | `#1c2024` | `#e3e4e5` | 12.87 |
| peek-ios-light.png | span.mask "[masked]" (3) | `#1c2024` | `#e3e4e5` | 12.87 |
| peek-ios-light.png | p.ifoot "Masking is best effort. This text is not kept…" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | button.ibtn "Refresh" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.phcap "2 · Off: how to turn on [remote] peek" | `#1b1f23` | `#b6bdba` | 8.66 |
| peek-ios-light.png | h2.navt "pm-1 screen" (2) | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | button.gb "Done" (2) | `#8a521c` | `#ffffff` | 6.36 |
| peek-ios-light.png | b "Screen peek is off for payments-api" | `#1c2024` | `#edeeee` | 14.10 |
| peek-ios-light.png | p "Peek shows the last lines of the PM's termina…" | `#1c2024` | `#edeeee` | 14.10 |
| peek-ios-light.png | pre "[remote] peek = true" | `#1c2024` | `#e3e4e5` | 12.87 |
| peek-ios-light.png | p "Then tap Try again. Optional" | `#1c2024` | `#edeeee` | 14.10 |
| peek-ios-light.png | code "peek_patterns" | `#1c2024` | `#edeeee` | 14.10 |
| peek-ios-light.png | button.ibtn "Try again" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.phcap "3 · Rate limited: the old text stays; Refresh…" | `#1b1f23` | `#b6bdba` | 8.66 |
| peek-ios-light.png | h2.navt "pm-1 screen" (3) | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | button.gb "Done" (3) | `#8a521c` | `#ffffff` | 6.36 |
| peek-ios-light.png | span.nv "Not verified" (2) | `#6b5100` | `#f1eee5` | 6.46 |
| peek-ios-light.png | span "Captured" (2) | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | b "12:31:07" (2) | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | b "Too many peeks" | `#1c2024` | `#f1eee5` | 14.13 |
| peek-ios-light.png | span.bt "build-host allows 6 a minute and one every 5 …" | `#1c2024` | `#f1eee5` | 14.13 |
| peek-ios-light.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" (2) | `#545b63` | `#f5f5f6` | 6.31 |
| peek-ios-light.png | pre "● Bash(npm run test:integration -- --filter w…" | `#1c2024` | `#f5f5f6` | 15.04 |
| peek-ios-light.png | button.ibtn "Refresh in 4 s" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | p.plabel "Entry: Peek on the PM card (plan-151 adds it)" | `#1a1e23` | `#aeb5b2` | 8.02 |
| peek-ios-light.png | span.id "pm-1" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | span.act "Working" | `#1c5ab6` | `#ffffff` | 6.59 |
| peek-ios-light.png | span.since "since 11:58 · 33 min" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | button.ir span "Peek at the screen" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | button.ir span.rv "on request" | `#545b63` | `#ffffff` | 6.88 |
| peek-ios-light.png | p.plabel "Loading: one read, nothing streams" | `#1c2024` | `#b9c0bd` | 8.85 |
| peek-ios-light.png | span.spinrow "Reading pm-1's screen on build-host…" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.plabel "No PM" | `#191e22` | `#b3bab7` | 8.50 |
| peek-ios-light.png | b "No PM to peek at" | `#1b1f23` | `#ffffff` | 16.58 |
| peek-ios-light.png | p "payments-api has no active PM right now." | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.plabel "No pane" | `#1c2024` | `#b9c0bd` | 8.85 |
| peek-ios-light.png | b "pm-1 has no terminal pane" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p "The host cannot read a screen for this PM, so…" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.plabel "Refused (forbidden, unauthorized)" | `#1a1d22` | `#aeb5b2` | 8.09 |
| peek-ios-light.png | b "build-host refused the peek" | `#a22119` | `#ffffff` | 7.59 |
| peek-ios-light.png | p "Only the host's operator may peek, and this i…" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.plabel "Older host" | `#1c2024` | `#b9c0bd` | 8.85 |
| peek-ios-light.png | b "build-host cannot peek yet" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p "Its Capstan has no peek command. Update Capst…" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p.plabel "No answer" | `#1c2024` | `#b9c0bd` | 8.85 |
| peek-ios-light.png | b "build-host did not answer" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | p "Nothing was read. Check the connection and tr…" | `#1c2024` | `#ffffff` | 16.39 |
| peek-ios-light.png | button.ibtn "Retry" | `#1c2024` | `#ffffff` | 16.39 |

### peek-ios-dark.png (58 elements, lowest 6.52:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| peek-ios-dark.png | p.phcap "1 · Peek sheet (large detent): the last 40 li…" | `#e6e8ea` | `#2e3238` | 10.49 |
| peek-ios-dark.png | h2.navt "pm-1 screen" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | button.gb "Done" | `#e0a45e` | `#1b1d20` | 7.76 |
| peek-ios-dark.png | span.nv "Not verified" | `#f0c95a` | `#343127` | 8.18 |
| peek-ios-dark.png | span "Captured" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | b "12:31:07" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | span "3 values masked" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" | `#a6adb4` | `#26282b` | 6.52 |
| peek-ios-dark.png | pre "● Bash(cstan inbox) 1 message waits for you (…" | `#eceef0` | `#26282b` | 12.71 |
| peek-ios-dark.png | span.mask "[masked]" | `#eceef0` | `#393b3e` | 9.66 |
| peek-ios-dark.png | span.mask "[masked]" (2) | `#eceef0` | `#393b3e` | 9.66 |
| peek-ios-dark.png | span.mask "[masked]" (3) | `#eceef0` | `#393b3e` | 9.66 |
| peek-ios-dark.png | p.ifoot "Masking is best effort. This text is not kept…" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | button.ibtn "Refresh" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.phcap "2 · Off: how to turn on [remote] peek" | `#e6e8ea` | `#2e3238` | 10.49 |
| peek-ios-dark.png | h2.navt "pm-1 screen" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | button.gb "Done" (2) | `#e0a45e` | `#1b1d20` | 7.76 |
| peek-ios-dark.png | b "Screen peek is off for payments-api" | `#eceef0` | `#2f3134` | 11.22 |
| peek-ios-dark.png | p "Peek shows the last lines of the PM's termina…" | `#eceef0` | `#2f3134` | 11.22 |
| peek-ios-dark.png | pre "[remote] peek = true" | `#eceef0` | `#393b3e` | 9.66 |
| peek-ios-dark.png | p "Then tap Try again. Optional" | `#eceef0` | `#2f3134` | 11.22 |
| peek-ios-dark.png | code "peek_patterns" | `#eceef0` | `#2f3134` | 11.22 |
| peek-ios-dark.png | button.ibtn "Try again" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.phcap "3 · Rate limited: the old text stays; Refresh…" | `#e6e8ea` | `#2e3238` | 10.49 |
| peek-ios-dark.png | h2.navt "pm-1 screen" (3) | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | button.gb "Done" (3) | `#e0a45e` | `#1b1d20` | 7.76 |
| peek-ios-dark.png | span.nv "Not verified" (2) | `#f0c95a` | `#343127` | 8.18 |
| peek-ios-dark.png | span "Captured" (2) | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | b "12:31:07" (2) | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | b "Too many peeks" | `#eceef0` | `#343127` | 11.18 |
| peek-ios-dark.png | span.bt "build-host allows 6 a minute and one every 5 …" | `#eceef0` | `#343127` | 11.18 |
| peek-ios-dark.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" (2) | `#a6adb4` | `#26282b` | 6.52 |
| peek-ios-dark.png | pre "● Bash(npm run test:integration -- --filter w…" | `#eceef0` | `#26282b` | 12.71 |
| peek-ios-dark.png | button.ibtn "Refresh in 4 s" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | p.plabel "Entry: Peek on the PM card (plan-151 adds it)" | `#eaecee` | `#2a2d32` | 11.67 |
| peek-ios-dark.png | span.id "pm-1" | `#eceef0` | `#1a1c1f` | 14.68 |
| peek-ios-dark.png | span.act "Working" | `#7eaffe` | `#1a1c1f` | 7.68 |
| peek-ios-dark.png | span.since "since 11:58 · 33 min" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | button.ir span "Peek at the screen" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | button.ir span.rv "on request" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-ios-dark.png | p.plabel "Loading: one read, nothing streams" | `#eceef0` | `#30343a` | 10.76 |
| peek-ios-dark.png | span.spinrow "Reading pm-1's screen on build-host…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.plabel "No PM" | `#e1e3e5` | `#2c3035` | 10.32 |
| peek-ios-dark.png | b "No PM to peek at" | `#eceef0` | `#1a1c1f` | 14.68 |
| peek-ios-dark.png | p "payments-api has no active PM right now." | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.plabel "No pane" | `#eceef0` | `#30343a` | 10.76 |
| peek-ios-dark.png | b "pm-1 has no terminal pane" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p "The host cannot read a screen for this PM, so…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.plabel "Refused (forbidden, unauthorized)" | `#dee0e1` | `#2a2d32` | 10.43 |
| peek-ios-dark.png | b "build-host refused the peek" | `#fea39b` | `#1a1c1f` | 8.88 |
| peek-ios-dark.png | p "Only the host's operator may peek, and this i…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.plabel "Older host" | `#eceef0` | `#30343a` | 10.76 |
| peek-ios-dark.png | b "build-host cannot peek yet" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p "Its Capstan has no peek command. Update Capst…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p.plabel "No answer" | `#eceef0` | `#30343a` | 10.76 |
| peek-ios-dark.png | b "build-host did not answer" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | p "Nothing was read. Check the connection and tr…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-ios-dark.png | button.ibtn "Retry" | `#eceef0` | `#1b1d20` | 14.52 |

### peek-mac-light.png (47 elements, lowest 5.82:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| peek-mac-light.png | h2.h3 "pm-1 screen · payments-api" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | span.nv "Not verified" | `#6b5100` | `#e9e6db` | 6.00 |
| peek-mac-light.png | span.note "Captured 12:31:07 · 40 lines · 3 values masked" | `#545b63` | `#f6f6f4` | 6.36 |
| peek-mac-light.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" | `#545b63` | `#ececeb` | 5.82 |
| peek-mac-light.png | pre "● Bash(cstan status) plan-15 in review round …" | `#1c2024` | `#ececeb` | 13.86 |
| peek-mac-light.png | span.mask "[masked]" | `#222629` | `#dbdcdb` | 11.09 |
| peek-mac-light.png | span.mask "[masked]" (2) | `#222629` | `#dbdcdb` | 11.09 |
| peek-mac-light.png | span.mask "[masked]" (3) | `#202528` | `#dbdcdb` | 11.26 |
| peek-mac-light.png | p.note "Masking is best effort. The text is not kept:…" | `#545b63` | `#f6f6f4` | 6.36 |
| peek-mac-light.png | button.btn "Refresh" | `#1c2024` | `#e4e5e3` | 12.97 |
| peek-mac-light.png | button.btn span.kbd "⌘R" | `#1c2024` | `#d4d6d4` | 11.21 |
| peek-mac-light.png | button.btn "Done" | `#ffffff` | `#8a521c` | 6.36 |
| peek-mac-light.png | p.plabel "Entry: Peek in the PM card header (plan-151)" | `#1a1e23` | `#aeb5b2` | 8.02 |
| peek-mac-light.png | span.id "pm-1" | `#1b1f23` | `#f5f5f3` | 15.19 |
| peek-mac-light.png | span.act "Working" | `#1c5ab7` | `#f5f5f3` | 6.02 |
| peek-mac-light.png | span.since "since 11:58 · 33 min" | `#535a62` | `#f5f5f3` | 6.40 |
| peek-mac-light.png | button.btn "Peek" | `#1b1f23` | `#e3e4e2` | 12.99 |
| peek-mac-light.png | p.plabel "Off" | `#1b1f23` | `#b6bdba` | 8.66 |
| peek-mac-light.png | b.h3 "Screen peek is off for payments-api" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.note "Peek shows the last lines of the PM's termina…" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | pre "[remote] peek = true" | `#1f2327` | `#ececeb` | 13.38 |
| peek-mac-light.png | p.note "Then choose Try again." | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | code "peek_patterns" | `#1f2327` | `#f6f6f4` | 14.61 |
| peek-mac-light.png | button.btn "Try again" | `#1c2024` | `#e4e5e3` | 12.97 |
| peek-mac-light.png | button.btn "Done" (2) | `#ffffff` | `#8a521c` | 6.36 |
| peek-mac-light.png | p.plabel "Loading" | `#191d22` | `#a6acab` | 7.35 |
| peek-mac-light.png | span.spinrow "Reading pm-1's screen on build-host…" | `#1b1f23` | `#f5f5f3` | 15.19 |
| peek-mac-light.png | p.plabel "Rate limited: the old text stays" | `#a8adaa` | `#1b1f23` | 7.28 |
| peek-mac-light.png | b "Too many peeks" | `#1c2024` | `#e9e6db` | 13.12 |
| peek-mac-light.png | span.bt "build-host allows 6 a minute and one every 5 …" | `#1c2024` | `#e9e6db` | 13.12 |
| peek-mac-light.png | button.btn "Refresh in 4 s" | `#545b63` | `#f6f6f4` | 6.36 |
| peek-mac-light.png | button.btn "Done" (3) | `#ffffff` | `#8a521c` | 6.36 |
| peek-mac-light.png | p.plabel "No PM · no pane" | `#a5aba8` | `#1b1f23` | 7.10 |
| peek-mac-light.png | b.h3 "No PM to peek at" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.note "payments-api has no active PM right now." | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | b.h3 "pm-1 has no terminal pane" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.note "The host cannot read a screen for this PM, so…" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.plabel "Refused (forbidden, unauthorized)" | `#191d22` | `#a8afac` | 7.57 |
| peek-mac-light.png | b.h3 "build-host refused the peek" | `#a22119` | `#f5f5f3` | 6.95 |
| peek-mac-light.png | p.note "Only the host's operator may peek, and this M…" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.plabel "Older host" | `#a8aeac` | `#1b1f23` | 7.35 |
| peek-mac-light.png | b.h3 "build-host cannot peek yet" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.note "Its Capstan has no peek command. Update Capst…" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.plabel "No answer" | `#a5aca9` | `#1b1f23` | 7.16 |
| peek-mac-light.png | b.h3 "build-host did not answer" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | p.note "Nothing was read. Check the connection and tr…" | `#1c2024` | `#f6f6f4` | 15.14 |
| peek-mac-light.png | button.btn "Retry" | `#1c2024` | `#e4e5e3` | 12.97 |

### peek-mac-dark.png (47 elements, lowest 6.52:1)

| PNG | Element | Foreground | Background | Ratio |
|---|---|---|---|---|
| peek-mac-dark.png | h2.h3 "pm-1 screen · payments-api" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | span.nv "Not verified" | `#f0c95a` | `#343127` | 8.18 |
| peek-mac-dark.png | span.note "Captured 12:31:07 · 40 lines · 3 values masked" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-mac-dark.png | div.ut-h "Terminal of pm-1 · untrusted text · scrolls" | `#a6adb4` | `#26282b` | 6.52 |
| peek-mac-dark.png | pre "● Bash(cstan status) plan-15 in review round …" | `#eceef0` | `#26282b` | 12.71 |
| peek-mac-dark.png | span.mask "[masked]" | `#e9ebed` | `#393b3e` | 9.40 |
| peek-mac-dark.png | span.mask "[masked]" (2) | `#e9ebed` | `#393b3e` | 9.40 |
| peek-mac-dark.png | span.mask "[masked]" (3) | `#e9ebed` | `#393b3e` | 9.40 |
| peek-mac-dark.png | p.note "Masking is best effort. The text is not kept:…" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-mac-dark.png | button.btn "Refresh" | `#eceef0` | `#2f3134` | 11.22 |
| peek-mac-dark.png | button.btn span.kbd "⌘R" | `#eceef0` | `#414346` | 8.53 |
| peek-mac-dark.png | button.btn "Done" | `#1a1206` | `#e0a45e` | 8.51 |
| peek-mac-dark.png | p.plabel "Entry: Peek in the PM card header (plan-151)" | `#dddfe1` | `#272a2f` | 10.78 |
| peek-mac-dark.png | span.id "pm-1" | `#ebedef` | `#1a1c1f` | 14.55 |
| peek-mac-dark.png | span.act "Working" | `#7eaffe` | `#1a1c1f` | 7.68 |
| peek-mac-dark.png | span.since "since 11:58 · 33 min" | `#a5acb3` | `#1a1c1f` | 7.44 |
| peek-mac-dark.png | button.btn "Peek" | `#eaecee` | `#2e3033` | 11.17 |
| peek-mac-dark.png | p.plabel "Off" | `#e9ebed` | `#2f3339` | 10.63 |
| peek-mac-dark.png | b.h3 "Screen peek is off for payments-api" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.note "Peek shows the last lines of the PM's termina…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | pre "[remote] peek = true" | `#eaecee` | `#26282b` | 12.48 |
| peek-mac-dark.png | p.note "Then choose Try again." | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | code "peek_patterns" | `#eaecee` | `#1b1d20` | 14.26 |
| peek-mac-dark.png | button.btn "Try again" | `#eceef0` | `#2f3134` | 11.22 |
| peek-mac-dark.png | button.btn "Done" (2) | `#191105` | `#e0a45e` | 8.58 |
| peek-mac-dark.png | p.plabel "Loading" | `#c5c7c9` | `#25282d` | 8.72 |
| peek-mac-dark.png | span.spinrow "Reading pm-1's screen on build-host…" | `#eaecee` | `#1a1c1f` | 14.42 |
| peek-mac-dark.png | p.plabel "Rate limited: the old text stays" | `#e9ebed` | `#1f2125` | 13.49 |
| peek-mac-dark.png | b "Too many peeks" | `#eceef0` | `#343127` | 11.18 |
| peek-mac-dark.png | span.bt "build-host allows 6 a minute and one every 5 …" | `#eceef0` | `#343127` | 11.18 |
| peek-mac-dark.png | button.btn "Refresh in 4 s" | `#a6adb4` | `#1b1d20` | 7.45 |
| peek-mac-dark.png | button.btn "Done" (3) | `#191105` | `#e0a45e` | 8.58 |
| peek-mac-dark.png | p.plabel "No PM · no pane" | `#e9ebed` | `#222529` | 12.88 |
| peek-mac-dark.png | b.h3 "No PM to peek at" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.note "payments-api has no active PM right now." | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | b.h3 "pm-1 has no terminal pane" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.note "The host cannot read a screen for this PM, so…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.plabel "Refused (forbidden, unauthorized)" | `#c5c7c9` | `#25282d` | 8.72 |
| peek-mac-dark.png | b.h3 "build-host refused the peek" | `#fea39b` | `#1a1c1f` | 8.88 |
| peek-mac-dark.png | p.note "Only the host's operator may peek, and this M…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.plabel "Older host" | `#e9ebed` | `#222529` | 12.88 |
| peek-mac-dark.png | b.h3 "build-host cannot peek yet" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.note "Its Capstan has no peek command. Update Capst…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.plabel "No answer" | `#e9ebed` | `#222529` | 12.88 |
| peek-mac-dark.png | b.h3 "build-host did not answer" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | p.note "Nothing was read. Check the connection and tr…" | `#eceef0` | `#1b1d20` | 14.52 |
| peek-mac-dark.png | button.btn "Retry" | `#eceef0` | `#2f3134` | 11.22 |

