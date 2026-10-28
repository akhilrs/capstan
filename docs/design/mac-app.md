# Capstan for Mac: design record (PM-298)

Status: revision 2, for the user's final approval. The user picked **direction A (Glass Workbench)**. B and C stay as a record of the options.

## User decisions (2026-10-28)

1. Direction A, Glass Workbench.
2. macOS glass and concentric rounded corners win over the web design system's "no glass, 4 px radii". This is the user's decision, not a deviation still to settle.
3. Agent branches and per-package plan details (dependencies, review state) will be in the protocol. They are designed fully on Agents (09) and Plans (10).
4. Merging from the app: yes. A Merge action on ready-to-merge items with a confirmation sheet and result states (15).
5. User sounds from `~/Library/Sounds` appear in the sound picker (14).
6. A host shows "no heartbeat" after 3 missed poll intervals (11).
7. New: on-demand messages to the PM at any time, with a conversation per project (16).

- **Design (all screens, light and dark):** https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8
- Direction A, Glass Workbench (built out): https://claude.ai/artifact/9eg9W5kh4oYACHQzxEHxFC
- Direction B, Glass Control Room: https://claude.ai/artifact/7HYNPhiX1xbSszcGwg8Tig
- Direction C, Glass Briefing: https://claude.ai/artifact/Nn9n7tzcgrNQS4pSpdUz5u
- Screenshots: `docs/design/mac-app/screens/` (one PNG per screen, light above dark) and `docs/design/mac-app/directions/`.

The artboards are HTML mockups of a native SwiftUI app. The build uses system materials and controls, not this HTML. Everything is generic: hosts, projects, agents and plans are example data, and nothing is tied to this repository.

## Brief

**Users.** The person who runs Capstan on one or more machines and wants to answer agents from a Mac without keeping a terminal open. They are experts in their own projects. They use the app many times a day, in short visits. The task that has to be fast: see what waits, read the exact text, answer or forward the item, and move on.

**Purpose.** A native window onto `ssh <host> cstan events` (protocol v1, `docs/remote-protocol.md`) and `cstan decide` (plan-59 `remote-decide`). The app reads everything and acts only through request-bound decisions.

**Tone.** Calm, exact and native.

**Axis scores (1 to 5).** Task intensity 4 (operate). Data density 3. Brand expression 2. Audience 2 (expert).

**References (traits only).**
- Docker Desktop: a resource sidebar, lists that show live status, and a detail pane. Used for the overall shape.
- Apple Mail on macOS 26: a three-column split view, a floating glass sidebar and a toolbar of capsule groups over the content. Used for the window chrome.
- Xcode Issues navigator: items grouped by kind with counts, and the selection drives an inspector. Used for the inbox grouping.
- 1Password for Mac: a confirmation sheet that names the exact item and the consequence. Used for the confirm sheet.
- macOS System Settings (Tahoe): grouped forms with switches and inline errors. Used for Settings.

**Constraints.** macOS 26 with SwiftUI: NavigationSplitView, inspector, sheets, MenuBarExtra, UserNotifications. SF Pro and SF Symbols. Light and dark follow the system. WCAG 2.2 AA. No text from an agent ever appears as rich text, as a link or in a notification. The poll interval is at least 1000 ms. There was no deadline; the user was away, so the PM asked for the work to go ahead without waiting.

**What was read first (design guide order).**
1. DESIGN.md: none found.
2. Design system: the Capstan system (Claude Design) is for the web site. It allows no glass and no shadow, uses 4 px radii, IBM Plex, and a rope-amber accent. The app keeps the accent, the voice and the rule that anything typed is mono. Materials and shapes follow macOS 26 instead, which is a deviation (see below).
3. Brand files: `docs/assets/` holds the logo. Fixed values: rope amber `#e0a45e` (dark) and `#8a521c` (light text-safe).
4. Current UI: `cstan dash` (a terminal UI, `docs/design/cstan-dash-v2.md`). It contributes state words with a glyph each, never colour alone.
5. Platform: SwiftUI on macOS 26; SF Symbols as icons; dark mode required.
6. Content extremes: commands up to 4096 bytes, reasons up to 1024, up to 32 prompt options of up to 256 bytes each, up to 64 projects per stream, and summaries up to 1024 bytes.
7. Users and tasks: see above.
8. Accessibility: WCAG 2.2 AA.
9. Scope: design only. The PM fixed the screen list.

## Directions offered

| | A Glass Workbench (built) | B Glass Control Room | C Glass Briefing |
|---|---|---|---|
| Catalog | Workbench + Conversation | Control Room | Briefing |
| Layout | sidebar, grouped inbox, inspector | dense table, host strip, inspector docked below | numbered queue rail, one decision on a stage |
| Density | medium | high | low |
| Audience | expert | expert operator, keyboard-first | general |
| Default appearance | follows the system | dark first | follows the system |

The directions differ on data density and on audience. A also takes C's option rows for permission prompts.

## Screens and states

| # | Artboard | Shows | States covered |
|---|---|---|---|
| 01 | Waiting for you | Main screen. Inbox grouped by kind (proposals, prompts, designs, messages, merges) with counts; badge 7; inspector for a command proposal | selected row, forwarded pill, terminal-only pill, widening flag, live status capsule |
| 02 | Permission prompt | Exact option rows (from C). The widening option is flagged "Changes permissions beyond this action · terminal only"; the option that takes text is terminal only | selected option, widening, terminal only, Esc |
| 03 | Confirm | Sheet before a destructive choice (deny), with the command, a note field and the default focus on Cancel | focus, destructive button |
| 04 | Decision outcomes | applied, forwarded ("Forwarded to pm-1, waiting"), needs_terminal, hash_mismatch (stale, refresh), not_waiting, repeat, sending, no_active_pm, exit 255 | loading, success, every error |
| 05 | Item inspectors | design approval (link as text, screens, approve or request changes), message reply, ready to merge with branch, target, commit and a Merge… button, text that failed its integrity check, text cut at 4096 bytes | untrusted text rules |
| 06 | Inbox states | empty, first-snapshot loading (skeleton rows), row default, hover, selected, keyboard focus, host offline, compact window under 900 px | empty, loading, hover, focus |
| 07 | Hosts and projects | per-host tables of projects; ready, no_key, silent, refused, unsupported and gone, each with its meaning; inspector with the fix | every project state |
| 08 | Add host | sheet: SSH host, extra project folders, live checks (connect, Capstan answers, projects found, first snapshot) | progress, warning |
| 09 | Agents | role, state, branch (with commits ahead of the HEAD branch and last commit time) and task, health (Working, Needs you, Stalled, Lost, Blocked, Paused, Idle); pause and resume an agent; pause the whole project | disabled Resume |
| 10 | Plans | progress bars (integrated, reported, assigned, not started); packages in integration order, each with its branch, review state (passed, changes requested with round, not reviewed), verify state and dependencies | in review, ready to merge, waits for you |
| 11 | Connection | status capsule: connecting, live with heartbeat, no heartbeat, reconnecting with backoff, resync, stopped. Banners for resync, reconnect, exit 255 and an unknown protocol version. Table of exit codes 0, 1, 2 and 255 and of the resync reasons | every connection state |
| 12 | Menu bar and notifications | menu bar extra with the count and a quick list of 5 items plus "2 more"; notifications that carry no agent text | highlighted row |
| 13 | Settings | hosts, SSH agent and key file, poll interval with the 1000 ms minimum shown as an inline error, heartbeat pulse, notification kinds | error, switches on and off |
| 14 | Settings › Notifications, sounds | default sound picker (None, the macOS alert sounds, then the user's own sounds from `~/Library/Sounds`) with a play/stop preview; a sound per kind (command proposal, permission prompt, design approval, message, ready to merge), each "Use default" or its own sound; a volume note (follows the system alert volume, no slider); Focus and Do Not Disturb followed, with an opt-in "Allow during Focus" (Time Sensitive) | menu open, preview playing, None (preview disabled), default vs own sound, user sound |
| 15 | Merge | confirmation sheet: branch, target (the project's HEAD branch), commit, review and verify status, whether it will push (a checkbox, on by default when the project has a remote); results: merged (and pushed), conflict (files named, nothing merged), refused because the working tree has changes, refused because not reviewed on this commit, merged but push failed, failed (nothing merged) | default focus on Cancel, every result |
| 16 | Messages to PM | "Message PM" compose button in every toolbar (⇧⌘N) opens a popover with a project picker; a conversation per project under "Messages to PM" in the sidebar, with an inline composer; your messages show Sending, Sent (on the host, not picked up), Delivered or Not sent (Retry); the PM's answers and its questions appear in the same thread as untrusted mono text | sending, sent, delivered, not sent, unread count |

**Notification sounds (added at the user's request).** The list is read at run time: the system alert sounds first, then the user's own sounds from `~/Library/Sounds` under a "Your sounds" heading. Nothing is hard-coded; the mockup shows the classic names as examples. A user sound that is later deleted falls back to the default sound. The preview uses the alert volume, the same as a real notification. Delivery goes through UserNotifications: a Focus or Do Not Disturb that silences Capstan silences its sound too, and the item still counts in the menu bar. "Allow during Focus" only asks macOS for the Time Sensitive level; the user's Focus settings decide. A kind switched off under General plays nothing, whatever sound it has.

**Merge.** A ready-to-merge item has a primary "Merge…" button. The sheet shows exactly what will happen, and the host checks the branch, commit and review again just before it merges. A refused or conflicting merge changes nothing in the checkout. The result appears in the inspector; a conflict offers "Message PM…".

**Messages to PM.** You can write to a project's PM at any time, not only in reply. The text is plain, up to 500 bytes. Delivery states map to the host's message states: Sent while it waits on the host, Delivered once the PM has taken it. A message the app could not hand over is kept with Retry. The PM's action-needed questions also appear in the inbox; answering in either place answers both.

## Components

Sidebar (glass, inset 8 px, with host and project tree), toolbar capsule group, segmented filter, status capsule, inbox row, kind group header, inspector header and meta list, **untrusted text block** (header with a warning glyph, mono body, no links), option row (normal, selected, widening, terminal), button (default, primary, destructive outline, destructive fill, disabled, sending), pill (green, yellow, blue, grey, accent; red pills are outlined), banner (red, yellow, blue, green, grey), table, progress bar, sheet, field with help and error, switch, menu bar menu, notification, merge sheet, conversation thread (your bubble in the accent, the PM's in an untrusted mono block), composer, compose popover, delivery state line.

## Tokens

Colour roles, light and dark:

| token | light | dark | role |
|---|---|---|---|
| win | `#f6f6f4` | `#1b1d20` | window content ground |
| glass | `rgba(252,252,250,.66)` | `rgba(48,51,56,.62)` | sidebar, toolbar capsules, sheets, menus; blur 30 px, saturate 170% |
| label | `#1c2024` | `#eceef0` | primary text |
| label2 | `#545b63` | `#a6adb4` | secondary text |
| sep | `rgba(28,32,36,.10)` | `rgba(255,255,255,.09)` | hairlines |
| fill / fill2 | 4% / 8% ink | 4% / 9% white | row hover, quiet buttons |
| accent | `#8a521c` | `#e0a45e` | the only action colour: primary button, selection, active nav |
| on-accent | `#ffffff` | `#1a1206` | text on the accent |
| red | `#a3221a` | `#ffa49c` | destructive, widening, error, lost and blocked |
| yellow | `#6b5100` | `#f0c95a` | stalled, reconnecting, stale |
| green | `#1d6b35` | `#6fd68b` | live, ready, passed, applied |
| blue | `#1d5bb8` | `#7fb0ff` | forwarded, in progress, reported; also the focus ring |

Type (SF Pro Text and SF Mono; the mockups use Inter and IBM Plex Mono as stand-ins): 26 large title (Briefing only), 20 inspector title (600), 15 headline (600), 13 body (400, 500, 600), 12 caption and mono. Tabular figures for ages and counts.

Spacing: a 4 px scale (4, 8, 12, 16, 20, 24, 32, 40).

Radius (concentric): window 22, sidebar and sheet 16 to 22, rows and blocks 10 to 12, inner controls 8, capsules at half their height.

Motion: 150 ms for hover and press; 250 ms for a state change; one heartbeat pulse of 200 ms on the live dot; sheets use the system animation. Under Reduce Motion the pulse stops and only the time text changes.

Depth: two levels. Glass floats over content (sidebar, toolbar, capsules, sheets, menus). Content itself is flat, with hairlines and tonal fills.

## Liquid Glass rules used

1. Glass only on the navigation layer that floats: the sidebar, toolbar capsule groups, the status capsule, sheets, the menu bar menu and notifications. It never goes on rows, tables or inspector content.
2. Content runs under the glass: the list scrolls beneath the toolbar capsules, and the sidebar is inset so content shows around it.
3. Shapes are concentric: inner radius = outer radius minus inset (window 22 → sidebar 16 at 6 to 8 px inset).
4. One accent, rope amber, used for selection and the primary action only. State colours never act as the accent.
5. Native idioms: NavigationSplitView (sidebar, content, inspector), toolbar, `.inspector`, `.sheet` with Cancel as the default focus on destructive sheets, MenuBarExtra, Settings scene, UserNotifications.
6. Untrusted text is never glass: it sits in a flat mono block with a labelled header.
7. Use the system materials (`.glassEffect`, `Material`) and system colours tuned by these tokens, not custom blur.

## Forbidden moves

- No glass on content rows or untrusted text blocks.
- No second accent; status colours never fill a primary action.
- Text from an agent is never rendered as a link, rich text or notification text.
- No gradient, except the system's own material.
- No state shown by colour alone: every pill, dot and row state has a word or a glyph.

## Verification

- **Screenshots:** 16 screens plus 3 directions, each in light and dark, rendered with Playwright from the artboard source (a local stub stands in for the canvas runtime and expands the light and dark loop). They are in `docs/design/mac-app/`.
- **Contrast (computed, not by eye):** revision 2 rechecked every changed artboard (01, 02, 03, 05, 07, 08, 09, 10, 14, 15, 16: 1,336 text elements, all AA, lowest 4.55:1). A script blended each text element's colour over its composited background in every artboard. 1,425 text elements were checked; all pass AA, and the lowest is 4.55:1. Three pairs failed on the first run and were fixed: red and yellow pills on tinted rows, a destructive button in dark mode, and notes over the wallpaper. The fix outlines red pills, gives a pill on a selected row the window colour, darkens light red and yellow, and lightens dark red. Placeholder text was not measured, and the backgrounds behind glass are modelled without blur.
- **Keyboard:** Tab was traversed on 01, 02, 03 and 13. Every stop shows a 3 px focus ring. The order runs sidebar → toolbar → inspector actions; the sheet traps focus (note → Cancel → Deny). In the mockup the list rows are not Tab stops; in SwiftUI the list is one stop and the arrow keys move the selection.
- **Widths:** a Mac window has no 375 or 768 px layout. The compact window (under 900 px: sidebar folded, inspector as a sheet, 640 × 480 minimum) is on artboard 06. The 375 and 768 px checks are **not applicable**.
- **Not verified:** real Liquid Glass refraction (the HTML imitates it with blur); SF Pro metrics (Inter stood in); VoiceOver; Dynamic Type sizes; the native SwiftUI build itself.

## Self-critique

Avoid list: purple-blue or default gradient **pass** (the only gradient is the system material). Glassmorphism everywhere **pass** (glass only on the floating layer, rule 1). Emoji or icon grids **pass**. Three identical cards in a row **pass**; the 3 × 3 grid on artboard 04 holds nine different outcomes, laid out as documentation. Centred hero with vague copy **pass**. Everything rounded and soft-shadowed **partial**: the radii follow macOS concentric shapes, and shadow is used only on the window and floating glass. Lorem ipsum **pass**. Generic Inter on white **pass**, with a note: SF Pro is the real face and Inter only stands in for it in the mockups.

Require list: at most two typefaces **pass** (SF Pro and SF Mono). Spacing scale **pass** (4 px). Restrained palette with one accent **pass**. Deliberate hierarchy and asymmetry **pass** (248 / flexible / 360 columns; the inspector carries the decision). Real content **pass**, using realistic example data. Motion with meaning **pass** (heartbeat pulse, state changes; Reduce Motion handled). Empty, loading, error, hover and focus states **pass** (artboards 04 and 06).

## Rule deviations

- **Design system vs macOS 26: decided by the user.** The Capstan system says no glass, no shadows, 4 px radii and IBM Plex. The user chose macOS glass and concentric rounded corners for the app. It uses Liquid Glass materials, window and floating shadows, concentric radii from 8 to 22 px, and SF Pro with SF Mono. The accent, voice and mono rule are kept.
- **T3 and T4:** body text is 13 px and the steps 12 to 13 are within 1.2×. These are the macOS type styles (body 13 pt, caption 12 pt). A larger text size comes from Dynamic Type.
- **K1:** the radius family is concentric (22, 16, 12, 10, 8 and capsule) rather than three fixed radii. Liquid Glass requires concentric shapes.
- **R6:** SF Pro and SF Mono are named; Inter and IBM Plex Mono stood in only in the HTML mockups.
- **I1 and imagery:** there is no imagery; the wallpaper behind each window is a flat neutral, so the glass is shown with little refraction.

## Design record changes

No DESIGN.md exists, and this package does not own one. Proposed additions for when one is written, or for the Capstan design system: a "Mac app" surface section that records the user's decision that macOS glass and concentric radii win over the web rules on this surface, and with the colour roles above, which add the `red`, `yellow`, `green` and `blue` state tokens and the glass material; the 13 / 15 / 20 type steps for the app; the concentric radius rule; the untrusted-text block as a named component; and the Liquid Glass rules 1 to 7. The web tokens (`bg`, `surface`, `fg`, `accent`) are unchanged.

## Open questions

Settled by the user: agent branch and package details come in the protocol; merging is allowed from the app; user sounds appear; "no heartbeat" after 3 missed poll intervals.

1. **Protocol work.** The new screens need protocol additions: a branch on `AgentItem`; per-package branch, dependencies, review and verify state on plans; a merge verb with the results merged, conflict, refused (dirty tree, not reviewed) and failed; and an on-demand message verb to the PM with sent and delivered states. These belong to a protocol plan, not to this design.
2. **Push default.** The merge sheet pushes by default when the project has a remote. Should the default instead come from a project setting?
3. **Confirming a widening option.** plan-59 refuses widening answers as `needs_terminal`, so the confirm sheet covers destructive choices (deny, merge, pause the project). If a later version allows widening answers remotely, should they get the same sheet with a typed confirmation?
4. **Opening a design link.** The link opens only after "Open in browser…". Should that show the full URL in a confirmation first?
5. **Sound names.** macOS 26 may ship different alert sound names than the classic ones in the mockup; the build lists what the system offers.
