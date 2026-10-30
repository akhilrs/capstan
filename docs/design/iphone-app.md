# Capstan for iPhone: design record (PM-379)

Status: **approved by the user on 2026-10-30** (revision 2; see Approval). The user picked **direction A, Glass Inbox**. B stays as a record of the options.

## Approval

The user approved this design (https://claude.ai/artifact/5VWuY49BdmY1LL49seX1ob) on 2026-10-30 and answered three open questions:

1. **Live Activity and widgets stay.** Without a server they show the last known count with its "as of" time (boards 13 and 15 already do).
2. **SSH key.** A Secure Enclave P-256 key (`ecdsa-sha2-nistp256`) is the default. An ed25519 Keychain key is offered as a fallback for hosts that accept only ed25519, with a warning that it can be backed up and restored to another device. Board 01 now has an "ed25519 fallback" sheet, reached from "Use an ed25519 key instead…" on the key screen; its safe default (focused) is "Keep the Secure Enclave key".
3. **Face ID before connecting is on by default** and can be turned off in Settings. Board 01 (the key screen) and board 14 (Settings) now show it on.

Revision 2 changed only boards 01 and 14 and their PNGs; contrast was re-run on both (all text passes AA, lowest 5.26:1).

## User decisions (2026-10-29)

1. Full parity with the Mac app (`docs/design/mac-app.md`, artboards 01 to 20, and the merge inspector).
2. The iPhone connects to hosts directly over SSH, like the Mac, with its own key in the Keychain / Secure Enclave. It uses the same remote protocol (`docs/remote-protocol.md`): `cstan events` for the snapshot and feed, `cstan decide` for decisions.
3. Apple's Human Interface Guidelines for iOS are followed strictly; the Capstan Liquid Glass look and tokens are kept and adapted to iOS materials.
4. A merge from the phone does **not** push by default (the Mac pushes by default when a remote exists).
5. Direction A, Glass Inbox (2026-10-30).

- **Design (all screens, light and dark):** https://claude.ai/artifact/5VWuY49BdmY1LL49seX1ob
- Direction A, Glass Inbox (picked): https://claude.ai/artifact/SzfFKQSbRXSq5NYsm6uTJ4
- Direction B, Glass Briefing: https://claude.ai/artifact/X1iAc52XUVWwxvnXj7goSd
- Screenshots: `docs/design/iphone-app/screens/` (one PNG per artboard, light row above dark row), `docs/design/iphone-app/directions/`, and `docs/design/iphone-app/focus-tab-check.png`.

The artboards are HTML mockups of a native SwiftUI app. The build uses system materials and controls, not this HTML. Hosts, projects, agents and plans are example data.

## Brief

**Users.** The person who runs Capstan on one or more machines and wants to answer agents away from the desk: on the sofa, in a queue, between meetings. Expert in their own projects. Many short visits a day, often one-handed, often from a notification. The task that must be fast: see what waits, read the exact text, answer or forward it, move on.

**Purpose.** A native iPhone window onto `ssh <host> cstan events` (protocol v1) and `cstan decide`. The app reads everything and acts only through request-bound decisions. A tap on a phone is never an approval: approvals and prompt answers are forwarded to the PM, who confirms with the person.

**Tone.** Calm, exact, native.

**Axis scores (1 to 5).** Task intensity 4 (operate). Data density 3 (a little lower than the Mac: one column). Brand expression 2. Audience 2 (expert).

**References (traits only).**
- Apple Mail on iOS 26: large-title lists, swipe actions, a floating glass tab bar, a bottom toolbar on the message. Used for the inbox and the detail.
- Apple Settings on iOS 26: inset grouped sections, switches, pickers with a check mark, footers that explain. Used for Settings, Hosts and forms.
- Termius for iOS: SSH hosts with a key per device and host-key fingerprints on first connection. Used for onboarding and host trust.
- GitHub Mobile: an inbox of review requests with a detail and a bottom action bar. Used for the item detail.
- Apple Wallet / Find My Live Activities: one number and a short state on the Lock Screen and in the Dynamic Island. Used for "N waiting".

**Constraints.** iOS 26 with SwiftUI: `TabView`, `NavigationStack`, `.sheet` with `presentationDetents`, `.swipeActions`, `.contextMenu`, `.confirmationDialog`, ActivityKit, WidgetKit, UserNotifications, CryptoKit `SecureEnclave.P256`, Keychain. SF Pro and SF Mono; SF Symbols. Light and dark follow the system; Increase Contrast, Reduce Transparency, Reduce Motion, Dynamic Type through AX5, VoiceOver, Full Keyboard Access. WCAG 2.2 AA. iPhone SE (3rd gen, 375 × 667 pt) to Pro Max (440 × 956 pt), portrait first; landscape where useful. No text from an agent ever appears as rich text, as a link, or in a notification, Live Activity or widget. Poll interval at least 1000 ms. No deadline was given.

**What was read first (design guide order).**
1. DESIGN.md: none found. `cstan design guide rules` read.
2. Design system: Capstan (Claude Design) is for the web site: no glass, 4 px radii, IBM Plex, rope-amber accent. The approved Mac record overrides it for app surfaces (user decision 2026-10-28). Its tokens are installed on the canvas.
3. Brand files: rope amber `#e0a45e` (dark) and `#8a521c` (light, text-safe). Logo in `docs/assets/`.
4. Current UI: the approved Mac design (16 artboards), the project overview (17 to 20) and the merge inspector. Patterns reused: grouped inbox, untrusted mono block, option rows, outcome banners, health words with glyphs, delivery states.
5. Platform: SwiftUI on iOS 26; SF Symbols; dark mode required; no web views for agent text.
6. Content extremes: commands up to 4096 bytes, reasons up to 1024, up to 32 prompt options of up to 256 bytes, up to 64 projects per stream, summaries up to 1024 bytes, messages to the PM up to 500 bytes.
7. Users and tasks: above.
8. Accessibility: WCAG 2.2 AA plus the iOS accessibility settings above.
9. Scope: design only; the PM fixed the screen list.

Tone words: calm, exact, native. The ui-ux-pro-max design-system query suggested a dark-only OLED style with a green accent and Plus Jakarta Sans; it was not used, because the approved Capstan tokens and SF Pro win.

## Directions offered

| | A Glass Inbox (picked) | B Glass Briefing |
|---|---|---|
| Catalog | Workbench + Conversation | Briefing |
| Tabs | Waiting · Agents · Plans · Messages · Hosts | Queue · Projects · Messages · Settings |
| Waiting | large-title grouped list, filter chips, push to a detail | one decision at a time on a stage card, queue below, Skip |
| Density | medium | low |
| Audience | expert | broader |

They differ on data density and audience. A maps 1:1 onto the Mac screens, which full parity asked for.

## Navigation map

```
TabView (floating glass tab bar, 5 tabs)
├─ Waiting (badge = count)        NavigationStack
│   ├─ list: filter chips All · Proposals · Prompts · Designs · Messages · Merges
│   │   ├─ swipe left: Deny… (proposals) · Message PM
│   │   └─ long press: preview + menu (Open, Copy command, Message PM, Open project, Deny…)
│   ├─ item detail (proposal, prompt, design, message, merge) → bottom action bar
│   │   ├─ sheet .medium: Confirm deny (Cancel default)
│   │   ├─ sheet small: Confirm a forward
│   │   └─ sheet .large: Merge
│   └─ toolbar: Message PM → New message sheet
├─ Agents                         NavigationStack
│   └─ agent detail → sheet: Pause (Cancel default)
├─ Plans                          NavigationStack
│   └─ plan detail (packages in integration order)
├─ Messages (badge = unread)      NavigationStack
│   ├─ conversation per project → thread + composer
│   └─ toolbar: New message (sheet .large with project picker)
└─ Hosts                          NavigationStack
    ├─ host → project overview (menu: Message PM, Plans, Agents, Refresh, Pause project…)
    ├─ project state detail (no_key, silent, refused, unsupported, gone) with the fix
    ├─ toolbar: Add a host (onboarding flow) · Settings
    └─ Settings → Notifications → sound per kind
First launch: Welcome → This iPhone's key → Add a host → Trust host key (sheet) → Checks
Outside the app: notifications (open the item), Live Activity and Dynamic Island, widgets (small, medium)
Deep links: capstan://item/<host>/<projectId>/<requestId>, …/agent/…, …/plan/…, …/conversation/<projectId>
```

## Screens and states

| # | Artboard | Shows | States |
|---|---|---|---|
| 01 | Onboarding and adding a host | Welcome; the Secure Enclave key (P-256, `ecdsa-sha2-nistp256`) with Copy and Share, the `authorized_keys` hint and Face ID on by default; the ed25519 Keychain fallback sheet with the backup warning; Add a host form; host-key trust sheet with the SHA256 fingerprint and the command to compare it; checks (SSH, host key, Capstan version and protocol, projects found, first snapshot); host key changed | progress, warning (no_key), error (key changed), disabled primary, default focus on Cancel |
| 02 | Waiting for you | Grouped by kind with counts, live status line, filter chips, swipe actions on a proposal, long-press preview and menu | filtered, swiped, menu open, forwarded pill, terminal-only pill, widening flag |
| 03 | Item details | Design approval (link shown as text, "Open in Safari…"), PM message with the reply composer, ready to merge (review and verify, Merge…), failed integrity check and text cut at 4096 bytes | untrusted-text rules, disabled send |
| 04 | Permission prompt and confirm | Exact option rows; widening flagged and terminal only; text options terminal only; all-terminal prompt; deny sheet at the medium detent with a note; forward confirmation | selected, disabled, focus |
| 05 | Decision outcomes | Sending; forwarded with a toast and the success haptic; changed since opened (`hash_mismatch`); every result: applied, forwarded, `needs_terminal`, `not_waiting`, repeat, `no_active_pm`, `limit_reached`, exit 255 | loading, success, every error |
| 06 | Inbox states | Empty, first snapshot (skeleton rows shaped like the real rows), one host reconnecting with items "as of 12:31" that cannot be answered, nothing reachable | empty, loading, stale, error |
| 07 | Agents | Health words with glyphs (Working, Needs you, Stalled, Lost, Blocked, Paused, Idle) and ages; segmented filter; detail with branch, commits ahead, last commit; Pause sheet; paused with Resume disabled because the project is paused | disabled |
| 08 | Plans | Progress bars with a text equivalent; packages in integration order with branch, review (passed, changes requested with round), verify, dependencies | not started |
| 09 | Project overview | Quiet project (Moving, nothing waits, agents now, recent outcomes in host time); a busy project with the project menu open | menu open, destructive item |
| 10 | Merge | Sheet at the large detent: branch, target, commit, review, verify, **Push switch off by default**; results: merged (not pushed) with Push main…, conflict with files named, refused (dirty tree, not reviewed on this commit), merged but push failed, failed, merged but not confirmed | default focus on Cancel, every result |
| 11 | Messages to PM | Conversations per project with unread; thread with Delivered, Sent (on the host, not picked up), Not sent with Retry; composer; New message sheet with the project picker and a 500-byte counter | sending states, keyboard up |
| 12 | Hosts and projects | Per host: live or reconnecting; projects in every state (Ready, No key, Silent, Refused, Unsupported, Gone) as words with glyphs; the fix for a state; host detail with Remove host… | every project state |
| 13 | Connection states | Status lines (connecting, live, no heartbeat after 3 intervals, reconnecting with backoff, resync, stopped); reconnecting banner over stale items; unknown protocol version; SSH failure with exit codes 0, 1, 2, 255 | every connection state |
| 14 | Settings and sounds | Hosts, key, Face ID (on by default), poll interval with the 1000 ms minimum as an inline error and a stepper, heartbeat pulse, notification kinds, a sound per kind, Time Sensitive; sound picker with preview and the user's own tones | error, switches, playing |
| 15 | Notifications, Live Activity, widgets | Lock Screen with the "7 waiting" Live Activity and notifications that carry only kind and project; Dynamic Island expanded, compact and minimal; small and medium widgets (count, nothing waits, by kind with stale note) | stale, empty |
| 16 | Sizes | iPhone SE (375 × 667, no Dynamic Island), Pro Max (440 × 956), Pro Max landscape as a split view | compact and regular width |
| 17 | Accessibility | Dynamic Type AX3 (rows stack; tab labels give way to the large-content viewer), Increase Contrast with Reduce Transparency (glass becomes opaque, separators and secondary text darken), VoiceOver labels | AX sizes, high contrast |

## HIG notes

- **Structure.** A `TabView` with five tabs (the HIG maximum) and one `NavigationStack` per tab, so each tab keeps its place. Large titles on every root; inline titles on pushed screens. Settings is reached from the Hosts toolbar, not a sixth tab.
- **Tab bar.** iOS 26 floating glass tab bar; it minimises on scroll (`tabBarMinimizeBehavior(.onScrollDown)`). Badges: Waiting count, Messages unread. Pushed detail screens keep the tab bar hidden while a bottom action bar shows, so the two never stack.
- **Actions.** One primary per screen in a bottom action bar inside the safe area (thumb reach). Destructive actions are outlined in red and always confirm in a sheet with **Cancel as the default** (`.keyboardShortcut(.cancelAction)` and initial focus). Approve is never a swipe, a notification action or a context-menu item: it needs the full text on screen.
- **Sheets and detents.** Confirm deny and Pause: `.medium` (content needs a note field and the untrusted text). Forward confirmation: a small custom detent. Merge, New message: `.large`. Sheets that show untrusted text use an opaque `presentationBackground`, never glass. Swipe down dismisses; a sheet with typed text asks before discarding.
- **Swipe actions.** Proposals: trailing Deny… (red, opens the deny sheet) and Message PM. Prompts and designs: Message PM only. Full swipe is off for Deny.
- **Context menus.** Long press on any row: a preview with the untrusted block and Open, Copy command, Message PM about this, Open project, and Deny… last and destructive. Project overview uses a pull-down menu from the "…" button.
- **Dynamic Type.** All text uses text styles (body 17, subheadline 15, footnote 13, title2 22, large title 34). At accessibility sizes rows switch to a vertical stack (`ViewThatFits`/`dynamicTypeSize.isAccessibilitySize`), metadata becomes one column, action bars stack and gain a solid background, and tab labels hide in favour of the large-content viewer on long press. Untrusted mono text wraps, never truncates in details.
- **Safe areas.** Content scrolls under the glass nav bar and tab bar; nothing interactive sits under the Dynamic Island or the home indicator. SE has no island and a 20 pt status bar; the tab bar sits 8 pt from the bottom edge.
- **Landscape.** Portrait first. Pro Max and Plus in landscape are regular width, so `NavigationSplitView` shows the list and the detail side by side, with insets for the island side. Other models rotate to the same stack.
- **Targets.** Every control is at least 44 × 44 pt; chips are 36 pt tall with a 44 pt hit area.
- **SF Symbols.** tray (Waiting), person.2 (Agents), list.bullet.rectangle (Plans), bubble.left (Messages), server.rack (Hosts), apple.terminal (proposal), checkmark.shield (prompt), photo (design), arrow.triangle.merge (merge), square.and.pencil (Message PM), exclamationmark.triangle (untrusted header). The mockups draw stand-ins.
- **Haptics.** `.success` when a decision is applied or forwarded; `.warning` on `hash_mismatch`, `needs_terminal`, `limit_reached`; `.error` on SSH failure; `.selection` on chip and option changes; a light impact when a swipe action arms. No haptic on heartbeats.
- **Increase Contrast, Reduce Transparency.** Glass becomes an opaque material, separators and secondary text darken (light) or lighten (dark), the accent deepens to `#6e3f12` / lightens to `#f0bb7c`.
- **Reduce Motion.** The heartbeat pulse stops; only the "3 s ago" text changes. Sheets and pushes use the system cross-fade.
- **VoiceOver.** A row reads kind, project, agent, age, then the first line of the text, with custom actions for Deny and Message PM. Pills and dots have words; icon-only buttons have labels ("Message PM", "More actions", "Add a host"). Untrusted blocks read their header first ("Command, untrusted text").
- **Notifications.** UserNotifications with categories per kind; the body says the kind and the project only. Tapping opens the item by deep link. Time Sensitive only when the user switches it on per kind; Focus decides.
- **Live Activity.** Starts when the first item waits and the user allowed it; shows the count and the kind breakdown; ends when nothing waits. Updated by the app while it runs and by background refresh. Remote push updates need a server, which Capstan does not have (open question 2).
- **Widgets.** Small (count) and medium (by kind with project names), from the last snapshot the app saved, with "as of" when stale.

## What differs from the Mac, and why

| Mac | iPhone | Why |
|---|---|---|
| Three-column split view with a sidebar of hosts | Tab bar with five stacks; hosts are a tab | Compact width; HIG top-level navigation |
| Inspector beside the list | Pushed detail with a bottom action bar | One column; thumb reach |
| Toolbar segmented filter | Horizontal filter chips under the large title | Fits narrow widths and Dynamic Type |
| Menu bar extra with quick list | Live Activity, Dynamic Island, widgets | The phone's glanceable surfaces |
| Merge sheet pushes by default when a remote exists | Push switch **off** by default | User decision for the phone; a mis-tap on a small screen should not reach a remote |
| SSH agent or key file | Secure Enclave P-256 key per iPhone (ed25519 Keychain key as a warned fallback), Face ID per connection on by default | No SSH agent on iOS; the default key cannot leave the device |
| Host key from `known_hosts` | Explicit trust sheet on first connection, block on change | The phone has no `known_hosts` the user manages |
| Sounds from `~/Library/Sounds` | System alert tones plus tones the user added | iOS sound sources |
| Hover states | Pressed states, swipe and long-press | Touch |
| Compact window at 640 × 480 | SE, Pro Max and landscape split | Device sizes |
| 13 pt body | 17 pt body (Dynamic Type default) | iOS text styles |
| Keyboard shortcuts (⇧⌘N) | Full Keyboard Access order and the same shortcuts on a hardware keyboard | iPhone with a keyboard is rare but supported |

## Tokens

The Mac colour roles are kept unchanged (`label`, `label2`, `sep`, `fill`, `fill2`, `accent`, `on-accent`, `red`, `yellow`, `green`, `blue`, `glass`). Added for iOS:

| token | light | dark | role |
|---|---|---|---|
| ground | `#efefec` | `#101214` | grouped background (system grouped background, tinted) |
| cell | `#ffffff` | `#1b1d20` | grouped cells, sheets with untrusted text, widgets |
| accent (Increase Contrast) | `#6e3f12` | `#f0bb7c` | accent under Increase Contrast |
| label2 (Increase Contrast) | `#3a4047` | `#c9ced3` | secondary text under Increase Contrast |

Type (SF Pro Text and SF Mono; Inter and IBM Plex Mono stand in): large title 34/700, title2 22/700, headline 17/600, body 17/400, subheadline 15, footnote 13, mono 15 (13 in notes). Tab labels follow the system tab bar. Tabular figures for counts and ages.

Spacing: 4 pt scale (4, 8, 12, 16, 20, 24, 32). Margins 16 pt (20 on Pro Max).

Radius (concentric to the display): display 55 (62 Pro Max), floating sheet 47 at an 8 pt inset, large sheet 38, grouped sections and option rows 22, untrusted blocks and fields 12, small controls 8, capsules at half their height (buttons 50 tall, chips 36, tab bar 62).

Motion: 150 ms press, 250 ms state change, one 200 ms heartbeat pulse, system sheet and push animations; Reduce Motion as above.

Depth: two levels, as on the Mac. Glass floats (nav bar buttons, tab bar, action bar buttons, menus, notifications, Live Activity); content is flat with tonal cells.

## Liquid Glass rules used (iOS)

1. Glass only on the floating layer: nav bar buttons, the tab bar, bottom action buttons, context menus, the Lock Screen surfaces. Never on rows, cells, untrusted text or sheets that show untrusted text.
2. Content scrolls under the nav bar and the tab bar.
3. Shapes are concentric with the display corners.
4. One accent, rope amber, for the primary action, the selected chip and the active tab only.
5. Native controls and materials (`.glassEffect`, `Material`, `presentationBackground`), not custom blur.
6. Reduce Transparency turns every glass surface opaque.

## Forbidden moves

- No glass on content rows or untrusted text.
- No second accent; state colours never fill a primary action.
- Agent text never in a notification, Live Activity, widget, link or rich text.
- Approve or answer never by swipe, notification action or widget.
- No gradient except the system material.
- No state by colour alone: every pill, dot and row state has a word or a glyph.

## Verification

- **Screenshots:** 17 artboards, each with every screen in light (top row) and dark (bottom row), rendered with Playwright from the artboard source (a local stub expands the light and dark loop). In `docs/design/iphone-app/screens/`.
- **Contrast (computed, not by eye):** a script blended each visible text element's colour over its composited background in all 17 artboards, light and dark: 2,348 text elements, all pass AA, lowest 4.88:1. The first run found 9 failures, fixed: stale rows were dimmed with opacity (3.35:1; now they keep full colour and say "as of 12:31" in the header and a pill), and an accent pill on a selected row (4.23:1; the pill now sits on the cell colour). Text covered by a sheet's scrim, disabled controls, placeholder keyboards and the annotation callouts were not measured; backgrounds behind glass are modelled without blur.
- **Keyboard (Full Keyboard Access order):** Tab traversed on 02, 04 and 14. Every stop shows a 3 px focus ring. Order on a detail: back → More actions → options → secondary → primary (`focus-tab-check.png`). The first run found the nav bar after the content; the mockup order was fixed. Destructive sheets put the initial focus on Cancel.
- **Widths:** 375 (SE), 402 (Pro), 440 (Pro Max) and 874 (Pro Max landscape) are drawn. 768 and 1440 px are **not applicable** to an iPhone app.
- **Not verified:** real Liquid Glass refraction; SF Pro metrics (Inter stood in); real Dynamic Type layout at AX5 (AX3 is drawn); VoiceOver and Full Keyboard Access on a device; Live Activity and widget sizes on a device; the SwiftUI build itself.

## Self-critique

Avoid list: purple-blue or default gradient **pass**. Glassmorphism everywhere **pass** (glass only on the floating layer). Emoji or icon grids **pass**. Three identical cards in a row **pass**. Centred hero with vague copy **pass** (the welcome is left-aligned and says what the app does and how it connects). Everything rounded and soft-shadowed **partial**: iOS 26 shapes are capsules and concentric radii; shadow only on floating glass. Lorem ipsum **pass**. Generic Inter on white **pass**, with the note that SF Pro is the real face.

Require list: at most two typefaces **pass** (SF Pro, SF Mono). Spacing scale **pass** (4 pt). Restrained palette with one accent **pass**. Deliberate hierarchy and asymmetry **pass** (large title, one primary per screen; the primary button is wider than the secondary). Real content **pass**. Motion with meaning **pass**. Empty, loading, error, hover (pressed), focus states **pass** (02, 05, 06, 13, 17).

Rules: T1 pass · T2 pass · T3 deviation (below) · T4 deviation · T5 pass · T6 pass · T7 pass · T8 n/a · C1 pass · C2 pass · C3 pass (computed) · C4 pass · C5 pass · C6 pass · C7 pass · L1 pass · L2 pass · L3 pass · L4 pass · L5 pass · L6 pass · L7 pass · L8 n/a (phone app, sizes drawn instead) · L9 pass · I1 deviation · I2 pass · I3 pass · I4 n/a · I5 n/a · I6 pass (the home screen "App" squares are labelled as other apps) · P1 to P6 pass · M1 to M5 pass · K1 deviation · K2 pass · K3 pass · K4 pass · K5 pass (rows wrap two lines, details never truncate) · K6 pass (pressed replaces hover) · K7 pass · R1 to R7 pass · A1 pass · A2 pass · A3 pass · A4 pass · A5 pass · A5b pass (AX3 drawn) · A6 pass.

## Rule deviations

- **Design system vs iOS 26: decided by the user** (as on the Mac). Glass, shadows on floating layers, concentric radii and SF Pro replace the web system's flat 4 px look; the accent, voice and mono rule are kept.
- **T3:** tab bar labels are the system's (about 10 to 11 pt); they enlarge through the large-content viewer, not Dynamic Type. Footnotes are 13 pt.
- **T4:** 13, 15 and 17 sit within 1.2× of each other; these are the iOS text styles (footnote, subheadline, body).
- **K1:** the radius family is concentric (55, 47, 38, 22, 12, 8, capsule), because Liquid Glass needs concentric shapes.
- **I1:** there is no imagery; the Lock Screen wallpaper is a flat neutral.
- **L8:** a phone app has no 1440 px layout; SE, Pro, Pro Max and landscape are drawn instead.

## Design record changes

No DESIGN.md exists, and this package does not own one. Proposed additions for when one is written: an "iPhone app" surface section beside the "Mac app" one, recording that iOS 26 materials win on this surface; the `ground` and `cell` tokens and the Increase Contrast variants above; the iOS type steps (34, 22, 17, 15, 13); the concentric radius list for the phone; the Liquid Glass rules for iOS; the forbidden moves (agent text never on a glanceable surface, approve never by gesture); and the push-off-by-default rule for merges from the phone. The web tokens and the Mac tokens are unchanged.

## Open questions

Answered on 2026-10-30 (see Approval): the Live Activity, the key type and the Face ID default. These stay open:

1. **Protocol work** (shared with the Mac): branch on agents, per-package review and verify, the merge verb and its results, and the message-to-PM verb with Sent and Delivered. The phone also needs the merge verb to take an explicit `--push` flag, since its default differs from the Mac.
2. **Background following.** iOS suspends SSH streams in the background. The design assumes background app refresh reconnects briefly to update the badge, the widget and notifications. Is that enough, or should notifications only come while the app is open?
3. **Fixing `no_key`.** The design does not name the host command that creates a project's operator key, because it is not documented in this repository. Which command should the fix screen show?
4. **Opening a design link.** As on the Mac, the link opens only after "Open in Safari…". Should it show the full URL in a confirmation first?
