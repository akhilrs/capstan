# Project overview: design

Claude Design: https://claude.ai/artifact/GzVdzrhxfPopiYcCMfAPKJ. It uses the Capstan design system and the Liquid
Glass look of the approved Mac app design (https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8). `base.css` is copied
unchanged from that canvas, and `overview.css` adds the overview parts using only its tokens. Every artboard is drawn
in light and dark.

The behaviour is in `docs/design/project-overview-proposal.md` (revision 3). This note lists what is drawn.

## Screens and states

| # | Artboard | Shows | States |
|---|---|---|---|
| 17 | Project overview, nothing waits | three-column window for payments-api: status strip (Moving), Agents now (3 working, View all 6), Plans in progress (approved, submitted, signed off but not merged), Recent outcomes backfilled from the host with host times including "yesterday 18:40", Show more; the inspector with nothing selected (project facts, the four freshness signals, Pause project…, Open plans, Open agents) | default, live |
| 18 | Status strip above waiting items | the strip (Needs attention: 2 working, 1 needs you, 1 stalled, 2 blocked, 1 lost, 1 idle), then the waiting groups unchanged, then a "Project" divider, then exceptions only (4 of 5, "+1 more needs attention · View all 8 agents"), with Plans and Recent outcomes folded into rows; the inspector shows the selected Lost agent with its task as untrusted text from dev-2 | selected, folded |
| 19 | Compact window, 640 × 480 | sidebar folded, one vertical scroll, no sideways scroll; a keyboard-focused row; larger text, where the strip wraps to 3 lines and agent rows stack | focus, larger text |
| 20 | Freshness and history states | see the two lists below | |

**Artboard 20, freshness:**
- Refreshing: Unknown, counts "–", rows dimmed as last known.
- One item pending: "current as of 14:05 · 1 item updating" and a dimmed row.
- Refresh failed: a banner with Retry.
- Host silent: no heartbeat for 3 intervals.

**Artboard 20, history:**
- loading;
- backfilled, where a report from yesterday reads "yesterday 18:52";
- session only (the reduced capability);
- error with Retry;
- truncated: "No outcomes in the last 500 events" plus the scan-limit note;
- gap divider: "History before 14:06 may be missing";
- the outcome inspector, with "Happened (host time)" and "Received (this Mac)" shown separately;
- the status words.

**State coverage:**
- Empty: the section texts are in the proposal, section 4. The empty inbox of screen 06 is replaced by artboard 17
  for a selected project.
- Loading: 20.
- Error: 20.
- Hover: as on screen 06, not redrawn.
- Focus: 19, plus the screen 06 rules.

## Kept, moved, changed, removed

- **Kept:** the waiting groups, rows, pills, inspectors (screens 09 and 10 for agent and plan rows), status capsule and
  connection banners.
- **Moved:** nothing.
- **Changed:**
  - With a project selected, the list column now shows the overview instead of the screen 06 empty state.
  - The status strip sits above the waiting groups.
  - The toolbar's "Message PM" compose button is left out of this view (see the open questions).
- **Removed:** the empty-state text "Nothing is waiting for you" with its tray icon, for a selected project only. The
  cross-project "Waiting for you" view keeps it.

## Rule deviations

- **C3/A1, dimmed last-known rows:** opacity .55 brings label2 text below 4.5:1. This is intended to mark stale data,
  and the state is also stated in words ("last known from 14:02", "updating…"). Trade-off: needs the user's decision.
  The alternative is to keep full contrast and rely on the words and the label only.
- **A1, not computed:** this canvas has no computed contrast check. All colours are the approved, AA-checked tokens.
  The new pairs are:
  - the outlined Unknown pill (label2 on the window colour);
  - the yellow gap line on the panel.

  Both are unverified.
- **I1/I2:** the symbols are inline stroke SVGs standing in for SF Symbols, as on the approved canvas.
- **K1:** the radius family is concentric, as decided for the Mac app.
- **Not verified:** no Playwright screenshots, Tab traversal or computed contrast were run for this canvas. The Design
  type asks not to render it without the user's request. VoiceOver labels are written into the markup but were not
  tested.

## Design record changes

None. No DESIGN.md exists, and this package does not own one. Proposed for a later Mac app surface record:
- the status strip, as a named component;
- the outcome row with host-time ages;
- the "pending" and "last known" dimming rule.

## Open questions

1. Should dimming (opacity .55) on last-known data stay, given that it fails AA on purpose? Or should it use full
   contrast with a "last known" label only?
2. The approved toolbar has "Message PM" (⇧⌘N) on every screen, but v1 has no free message to the PM. Should the
   overview hide it, as drawn, or keep it disabled with an explanation?
3. Should a folded section (18) remember being expanded per project?
4. `feed recent` and the run's build slot need the architect before phase 3 (proposal, section 7).

## Approval

Approved by the user on 2026-10-29 (https://claude.ai/artifact/GzVdzrhxfPopiYcCMfAPKJ, screens 17 to 20, as drawn). PM defaults, to be confirmed by the user: last-known rows dimmed at opacity .55 with the words; Message PM stays in the toolbar on this view; a folded section remembers its state per project.
