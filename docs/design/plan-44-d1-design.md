# plan-44 d1: the auto-approve window in `cstan status`, `cstan watch` and the dash header

Claude Design canvas: <https://claude.ai/artifact/LUbRSUyVA4qaZ2YX8Jog9F> (built on the Capstan design system), artboard **A**. Screenshot: [`plan-44-d1-design-a.png`](plan-44-d1-design-a.png).

## Approval

The user approved direction A, recorded by the PM with `cstan design approve`. The user's choice, verbatim from the picker: "A: one line (Recommended)". That covers the one-line status and watch text (`AUTO-APPROVE 42 min left, Operator and workers (win-12)`), the dash tab (`AUTO-APPROVE 42m`), and FULL AUTO being the last tab dropped at narrow widths. This document specifies only direction A.

The machine-readable table is [`plan-44-d1-design-strings.json`](plan-44-d1-design-strings.json). p7 loads it with `include_str!`.

## Input

Only the p3 window record, `operator.window` in the status JSON:

| field | used as |
| --- | --- |
| absent | the **none** case |
| `operatorActive: true` | the **open** case |
| `operatorActive: false` | the **Operator part ended** case: Operator proposals are no longer auto-approved, worker permission prompts still are, until `expiresAt` |
| `remainingSeconds` | `{left}` = `ceil(remainingSeconds / 60)`. An open window always has `remainingSeconds >= 1`, so 1..60 → 1, 61 → 2, 2520 → 42 |
| `windowId` | `{windowId}`, for example `win-12` |

`kind`, `minutes`, `source`, `createdAt` and `expiresAt` are not shown. The record has no approval count and no rule text, so none is designed for.

## The table

| id | template | colourRole | maxWidth 80 | maxWidth 40 | truncate |
| --- | --- | --- | --- | --- | --- |
| `line.window.open` | `AUTO-APPROVE {left} min left, Operator and workers ({windowId})` | warn | 80 | 40 | end |
| `line.window.operatorEnded` | `AUTO-APPROVE {left} min left, workers only; Operator ended ({windowId})` | warn | 80 | 40 | end |
| `status.window.none` | `AUTO-APPROVE off: no window open` | dim | 32 | 32 | none |
| `dash.window.open` | `AUTO-APPROVE {left}m` | warn | 17 | 0 | none |
| `dash.window.operatorEnded` | `AUTO-APPROVE {left}m workers` | warn | 25 | 0 | none |

Rules for reading the table:

- `maxWidth` is the most terminal cells the rendered string may take at that terminal width. For a dash tab it counts the tab text only; the tab adds 4 cells of edge (`┤ ` and ` ├`). `0` means the string is not drawn at that width: the dash draws no header under 60 columns.
- `truncate: "end"` cuts with the dash's `truncate` (`dash/src/view/format.rs`): keep `maxWidth - 1` cells, then `…`. `"none"` means never cut: a tab is shown whole or dropped whole, and the none line fits every width.
- The widest real values are `{left}` = 480 and an id such as `win-1234`: `AUTO-APPROVE 480 min left, workers only; Operator ended (win-1234)` is 66 cells, and `AUTO-APPROVE 480m workers` is 25.
- Every window string contains `AUTO-APPROVE` and none contains `full auto`. The full-auto strings `FULL AUTO {minutes}m` (dash) and `FULL AUTO ON: {n} minutes left` (watch) are unchanged and are not in the table.
- Colour is never the only signal: open and ended differ in their words.

## Where the line goes in `cstan status` and `cstan watch`

The window line comes right after the `FULL AUTO ON` line and before the `grant ...` lines (`operator_lines` in `rust/crates/cstan/src/watch.rs`). `cstan status` prints `status.window.none` when there is no window; `cstan watch` prints nothing then, so an idle watch stays quiet.

### 80 columns

watch, open (`line.window.open`):

```text
agents: designer-6 (Developer, active), pm-1 (PM, active)
messages: none unresolved
AUTO-APPROVE 42 min left, Operator and workers (win-12)
grant g-7 (timed) "cargo test" 18 min left
```

watch, Operator part ended (`line.window.operatorEnded`):

```text
agents: designer-6 (Developer, active), pm-1 (PM, active)
messages: none unresolved
AUTO-APPROVE 42 min left, workers only; Operator ended (win-12)
```

watch, with full auto on (line order):

```text
FULL AUTO ON: 12 minutes left
AUTO-APPROVE 42 min left, Operator and workers (win-12)
grant g-7 (timed) "cargo test" 18 min left
```

status, three separate outputs: open with 45 s left (`line.window.open`), Operator part ended (`line.window.operatorEnded`), none (`status.window.none`):

```text
AUTO-APPROVE 1 min left, Operator and workers (win-12)
AUTO-APPROVE 42 min left, workers only; Operator ended (win-12)
AUTO-APPROVE off: no window open
```

### 40 columns

open (`line.window.open`), 42 min and 1 min:

```text
AUTO-APPROVE 42 min left, Operator and …
AUTO-APPROVE 1 min left, Operator and w…
```

Operator part ended (`line.window.operatorEnded`):

```text
AUTO-APPROVE 42 min left, workers only;…
```

none, status only (`status.window.none`):

```text
AUTO-APPROVE off: no window open
```

The cut keeps the minutes and the words that tell the two states apart: "Operator and" against "workers only".

## The dash header tab

### Rules

- **Place.** The left tabs read: project, run, PAUSED (when paused), FULL AUTO, AUTO-APPROVE, grants. The window tab sits right after FULL AUTO.
- **Drop order** when the top line is too narrow: the right tabs go first, as today (interval, then clock). Then the left tabs go in this order: project, run, PAUSED, grants, AUTO-APPROVE, FULL AUTO. The two tabs that say guards are relaxed go last.
- **What changes from today.** `top_border` (`dash/src/view/border.rs`) now drops the last left tab first. With the real 33-character project id, at 80 columns that already hides `FULL AUTO 12m`. The new drop order is the one change to current behaviour. The full-auto text itself does not change.
- **Never cut.** A tab is whole or absent.
- **No window.** No tab, and the header is exactly today's.

### 80 columns

today, no window (unchanged):

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├┤ 14:02:11 ├─╮
```

open (`dash.window.open`), 42 min and 1 min:

```text
╭─ cstan dash ─┤ run active ├┤ AUTO-APPROVE 42m ├──────────────────────────────╮
╭─ cstan dash ─┤ run active ├┤ AUTO-APPROVE 1m ├───────────────────────────────╮
```

Operator part ended (`dash.window.operatorEnded`):

```text
╭─ cstan dash ─┤ run active ├┤ AUTO-APPROVE 42m workers ├──────────────────────╮
```

full auto, window and grants together (open, then Operator part ended):

```text
╭─ cstan dash ─┤ run active ├┤ FULL AUTO 12m ├┤ AUTO-APPROVE 42m ├┤ grants 2 ├─╮
╭─ cstan dash ─┤ FULL AUTO 12m ├┤ AUTO-APPROVE 42m workers ├┤ grants 2 ├───────╮
```

### 60 columns (the dash minimum), together

```text
╭─ cstan dash ─┤ FULL AUTO 12m ├┤ AUTO-APPROVE 42m ├───────╮
```

### 40 columns, every case

The dash refuses to lay out under 60 columns and prints its placeholder cut to the width, so neither tab is drawn (`maxWidth` 0). `cstan watch` is the 40-column view.

```text
terminal too small (need 60x16, have 40…
```

## States

| state | status / watch | dash |
| --- | --- | --- |
| empty (no window) | status: `status.window.none`; watch: nothing | no tab |
| loading (no status read yet) | nothing | no tab |
| error (link down) | the command's own error | the last good frame under the NO LINK chip, tab included; the minutes stop until the link is back |
| hover, focus | not interactive | not interactive |

## Decisions (formerly open questions, resolved with the approval of A)

1. **The new tab drop order is part of this design and ships in p7.** It also changes when `run active` and the project id disappear: at 80 columns, the project id goes as soon as a window opens. That is accepted.
2. **No line when a window closes.** `cstan watch` prints nothing when a window ends, the same as full auto today; the line simply disappears.
3. **`kind: session` windows show minutes left** like every other kind, from `remainingSeconds`. The session cap still has an `expiresAt`, so minutes left is accurate.
