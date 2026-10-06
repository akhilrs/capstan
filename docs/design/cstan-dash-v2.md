# `cstan dash` v2: visual redesign spec

**Status:** approved by the user and implemented (see section 9 for the decisions, the overlay spike result and where the build differs from the proposal below). Sections 1 to 8 are the approved proposal; the rendered screens that the code actually produces are the golden files in `test/golden/` and the pty captures in `docs/design/cstan-dash-v2-frames.md`, and they win where the two differ.
**Plan of record:** `decisions/DEC-006-cstan-dash.md` (the dashboard exists in `src/dash/`; this spec replaces its look, not its behaviour).
**Reference look:** btop (rounded boxes, titles and tabs embedded in the border, hotkey numbers, gradient meters, braille graphs, selected-row bar, clock and interval in the top border).

## 0. Summary

The v1 dashboard draws plain text lines under numbered titles. The redesign changes three things and keeps everything else:

1. **Every panel becomes a rounded box** with its title, hotkey number and tabs in the top border, a counter and tabs in the bottom border, and a scroll thumb in the right border. The focused box is heavy-lined and brighter.
2. **The layout fills the terminal.** Panels are sized from their content (fill, then stretch), not divided evenly. Spare rows go to graphs and a selected-item detail area, so there is no dead space and no false `+N more`.
3. **Real visual elements** replace text counts: a worker meter, per-agent state glyphs and activity bars, stacked stage bars in the pipeline, braille graphs of history kept in a client-side ring buffer, a selected-row bar, tables with column headers.

Help, the retry confirmation and the observe screen become centred floating boxes over the dashboard.

**Kept from DEC-006 (nothing here weakens it):**

| DEC-006 guarantee | In this spec |
| --- | --- |
| Read-only data path: only `callDaemon(..., "status")` plus the three confirmed actions | Unchanged. No new route, no new field beyond `supervisionState`/`peek`. Every visual element is derived from the `status` response or from a client-side ring buffer (section 5) |
| Confirm flow: action key, then a second `y` (not sooner than 300 ms), target id captured at open, a state change cancels | Unchanged; the prompt moves from the footer line to a dialog (section 2.2) and no longer truncates (bug B6) |
| `NO_COLOR` / `--no-color` / `TERM=dumb`: meaning never carried by colour alone | Every state has a word and a glyph; focus, selection and meters survive without colour (section 3.3) |
| Reduced motion (`--reduced-motion`, `CSTAN_REDUCED_MOTION=1`): no spinner, no fade, no bell | Same, plus no overlay animation (there is none). Graphs are data and stay (section 3.4) |
| Non-TTY refusal, `EXIT.usage` | Unchanged (`src/dash/terminal.ts`, `src/dash/run.ts`) |
| Keys: `Tab`/`Shift+Tab`, `1`..`5`, `↑↓`/`jk`, `o`, `y` `s` `c`, `?`, `p`, `r`, `q`/`Ctrl+C`, `Esc`/`q` closes the observe screen | Unchanged. One optional addition is flagged in section 8 (`-`/`+` for the poll interval) |
| Minimum size 60x16, two columns from 100 columns | Unchanged |

**How to read the mockups.** They are produced by a script, not typed by hand, so each one is exactly the stated width and height in code points (every glyph used is one terminal cell wide in a normal UTF-8 terminal). Colour cannot appear in Markdown; each mockup is followed by a colour note. Sample data is invented but shaped like real status data: a PM, a Supervisor, three workers (one stalled), a stuck message, an escalated finding, a degraded health reason. The braille graph shapes are synthetic. The observe text in 2.3 is invented; real text is whatever the agent's pane shows.

Conventions in the mockups:

- `▌` at the left edge of a row is the selected row. In the real render the whole row also gets the selected background.
- `⠋` in the first column of an agent row is the spinner frame (an agent that is working). It animates through `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`.
- `▲` marks "needs attention". `●` is idle or fine. `○` is ended.
- `█` in the right border is the scroll thumb (section 9.4). `n/total` in the bottom border is the cursor position.

## 1. Mockups

### 1.1 80x24 (compact, one column, minimum comfortable size)

Header is 4 rows here because the degraded reason does not fit on one line (it wraps to a second line instead of being cut off). Agents and queue show 3 rows each with the thumb; the attention-first sort puts the stalled agent and the stuck message on top so they are never the hidden rows. The pipeline collapses to the flow line plus a one-line history summary; findings collapse to one row and no column header.

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED                                       workers ██████░░ 3/4   ● 1s │
│   lost contact with developer-2 (pane p4 not responding)                     │
╰──┤ supervision on ├──────────────────────────────────────────────────────────╯
╭─¹agents──────────────────────────────────────────────┤ 5 active ├┤ 1 ended ├─╮
│   AGENT                        ROLE       GEN STATE   ACTIVITY   AGE  Q PANE │
│ ▲ developer-2                  Developer  g1  STALLED ░░░░░░░░ 3m41s  2 p4   ▐
│ ⠋ developer-1                  Developer  g2  working ████████    3s  1 p3   ▐
│ ⠋ designer-1                   Designer   g1  working ████████    1s  1 p5   │
╰─┤ working: inferred ├────────────────────────────────────────────────┤ 1/5 ├─╯
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮
│ reported 6  ──▶  review 5  ──▶  integrated 3                                 │
│ msgs ⣦⣤⣶⣤⣴⣤⣶⣴⣶⣦⣶⣶⣶⣶ 5   working ⣤⣴⣶⣶⣶⣶⣶⣶⣶⣶ 3   last: running 7c1e0f2a 5s     │
╰──────────────────────────────────────────────────────────────────────────────╯
┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
┃    SEQ TO           STATE     AGE N DETAIL                                   ┃
┃▌▲  #41 developer-2  unacked 3m41s ● ! no acknowledgement after 3 attempts    ▐
┃ ●  #42 developer-2  queued  3m10s · deferred: recipient busy                 ▐
┃ ●  #44 developer-1  sent       8s ●                                          ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/5 ├━┛
╭─⁴findings───────────────────────────────────────────────┤ 1 needs operator ├─╮
│ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress after 2 interventions   ▐
╰──────────────────────────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  y retry  s skip  c cancel  tab focus  p pause  ? help  q quit       
```

Colour note: border colours per panel (agents green, pipeline violet, queue brown-red, findings olive); the focused queue box is brighter and heavy-lined. `▲ DEGRADED` and the reason are red. `STALLED` and the `▲` on its row are red; `working` is green; `idle` is grey-green. The worker meter fills green, then yellow, then red as it nears the limit (3 of 4 is yellow-green). The queue `unacked` row is red.

### 1.2 120x36 (two columns)

Left column: agents, then pipeline. Right column: queue (with a selected-message detail area and two graphs), then findings. Every box is full of content; the rows left over after the tables went to the graphs.

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED  lost contact with developer-2 (pane p4 not responding)         workers █████████░░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 5 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
│   AGENT         ROLE       GEN STATE   ACTIVITY   AGE  Q │┃    SEQ TO           STATE     AGE N DETAIL               ┃
│ ▲ developer-2   Developer  g1  STALLED ░░░░░░░░ 3m41s  2 │┃▌▲  #41 developer-2  unacked 3m41s ● ! no acknowledgemen… ┃
│ ⠋ developer-1   Developer  g2  working ████████    3s  1 │┃ ●  #42 developer-2  queued  3m10s · deferred: recipient… ┃
│ ⠋ designer-1    Designer   g1  working ████████    1s  1 │┃ ●  #44 developer-1  sent       8s ●                      ┃
│ ● pm-1          PM         g1  idle    █████░░░   12s  1 │┃ ●  #45 designer-1   sent       2s ●                      ┃
│ ● supervisor-1  Supervisor g1  idle    ███████░    6s  0 │┃ ●  #46 pm-1         queued     1s ·                      ┃
│ ○ developer-0   Developer  g1  ended              14m  0 │┃ ── selected ───────────────────────────────────────────… ┃
╰─┤ working: inferred ├────────────────────────────┤ 1/5 ├─╯┃ #41 to developer-2  unacked  queued 3m41s ago            ┃
╭─²pipeline─────────────┤ reported > review > integrated ├─╮┃ message 0192f4c1-3a7e-7b2d  queued 14:58:42  notified 1… ┃
│ reported 6  ──▶  review 5  ──▶  integrated 3             │┃ stuck: no acknowledgement after 3 attempts. Retry sends… ┃
│                                                          │┃ unresolved messages ─ now 5 ─ max 6 ─ since dash start   ┃
│ reports      ████████████▒▒     accepted 5  rejected 1   │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⠀⠀ ┃
│ reviews      ████████▓▓▓▒▒▒     passed 3  started 1  fi… │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⢀⠀⡀⠀⣀⠀⢀⠀⡀⠀⣀⠀⢀⠀⣀⢀⣀⡀⣸⣀⣀⣀ ┃
│ integrations █████████▓▓▓▓▓     merged 2  running 1      │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢠⠀⢠⠀⡄⠀⣤⠀⢠⠀⡄⠀⣤⠀⢠⣤⣤⢠⣼⡄⣼⣤⣧⢠⣿⡄⣼⣤⣧⢠⣿⣤⣼⣤⣿⣼⣿⣧⣿⣿⣿⣿ ┃
│                                                          │┃ ⡄⠀⣤⠀⢠⠀⡄⠀⣤⠀⢠⣤⣤⢠⣼⡄⣼⣤⣧⢠⣿⡄⣼⣤⣧⣤⣿⣤⣼⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ STAGE       STATE    WHO / COMMIT                    AGE │┃ ⣷⢰⣿⡆⣾⣶⣷⢰⣿⣶⣾⣿⣿⣾⣿⣷⣿⣿⣿⣾⣿⣷⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration running  7c1e0f2a                         5s │┃ ⣿⣾⣿⣷⣿⣿⣿⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      started  developer-1 r2                  40s │┃ oldest unresolved message (age) ─ now 3m41s ─ since das… ┃
│ report      accepted designer-1 8bf60fa               1m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣀⣤⣤⣤⣤⣤ ┃
│ report      accepted developer-1 9abf487              3m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣀⣠⣤⣴⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   5d02b9c4                         4m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣠⣤⣤⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      findings developer-1 r1                   6m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣀⣤⣤⣴⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      passed   designer-1 r1                    9m │┃ ⠀⠀⠀⣀⣤⣶⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣠⣤⣴⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      rejected developer-1 45fe0de             11m │┃ ⣴⣶⣿⣿⣿⣿⣀⣀⣤⣤⣶⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted developer-2 fb4a3d5             15m │┃ ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   31aa8e07                        16m │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/5 ├━┛
│ review      passed   developer-2 r1                  18m │╭─⁴findings───────────────────────────┤ 1 needs operator ├─╮
│ review      passed   developer-0 r1                  24m ││   ID     TARGET       SEV  STATE     INT REASON          │
│ report      accepted developer-0 5451671             26m ││ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress af… │
│ report      accepted developer-0 3f1d9a0             29m ││ ● 0192a4 developer-1  low  open      0/2 scope drift: e… │
╰─────────────────────────────────────────────────┤ 1/14 ├─╯╰──────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  tab focus  1-4 panel  y retry  s skip  c cancel  o observe  p pause  r refresh  ? help  q quit              
```

Colour note: as 1.1. The braille graphs are coloured by height: low = green, middle = yellow, top = red (the unresolved graph peaks at 6 and therefore reaches amber). Pipeline stage bars: `█` green (good outcome), `▓` yellow (in progress), `▒` red (bad outcome). The `selected` rule and its three detail lines are dim; the reason line is yellow when the message is a delivery problem.

### 1.3 160x45 (two wider columns, all optional columns shown)

Same structure as 1.2. The extra width shows the `PANE` column and the full texts; the extra height turns into a working-agents graph under the agents table and two tall graphs in the queue box. The pipeline shows all 14 items and the `1/14` counter.

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────────────────────────────────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED  lost contact with developer-2 (pane p4 not responding)                                                 workers █████████░░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────────────────────────┤ 5 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
│   AGENT                        ROLE       GEN STATE   ACTIVITY   AGE  Q PANE │┃    SEQ TO           STATE     AGE N DETAIL                                   ┃
│ ▲ developer-2                  Developer  g1  STALLED ░░░░░░░░ 3m41s  2 p4   │┃▌▲  #41 developer-2  unacked 3m41s ● ! no acknowledgement after 3 attempts    ┃
│ ⠋ developer-1                  Developer  g2  working ████████    3s  1 p3   │┃ ●  #42 developer-2  queued  3m10s · deferred: recipient busy                 ┃
│ ⠋ designer-1                   Designer   g1  working ████████    1s  1 p5   │┃ ●  #44 developer-1  sent       8s ●                                          ┃
│ ● pm-1                         PM         g1  idle    █████░░░   12s  1 p1   │┃ ●  #45 designer-1   sent       2s ●                                          ┃
│ ● supervisor-1                 Supervisor g1  idle    ███████░    6s  0 p2   │┃ ●  #46 pm-1         queued     1s ·                                          ┃
│ ○ developer-0                  Developer  g1  ended              14m  0 -    │┃ ── selected ───────────────────────────────────────────────────────────────… ┃
│ working agents (inferred) ─ now 3 of 4 slots ─ since dash start              │┃ #41 to developer-2  unacked  queued 3m41s ago                                ┃
│ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │┃ message 0192f4c1-3a7e-7b2d  queued 14:58:42  notified 14:58:44               ┃
│ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │┃ stuck: no acknowledgement after 3 attempts. Retry sends it again; skip or c… ┃
│ ⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃ unresolved messages ─ now 5 ─ max 6 ─ since dash start                       ┃
│ ⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ ┃
│ ⠀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢠⠀⠀⠀ ┃
│ ⠀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⠀⠀⠀ ┃
│ ⠀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢰⠀⢰⠀⡆⠀⣶⠀⢰⠀⡆⠀⣶⠀⢰⠀⣶⢰⣶⡆⣾⣶⣶⣶ ┃
│ ⣀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣀⣀⣸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⢀⠀⡀⠀⣀⠀⢀⠀⡀⠀⣀⠀⢀⣀⣀⢀⣸⡀⣸⣀⣇⢀⣿⡀⣸⣀⣇⢀⣿⣀⣸⣀⣿⣸⣿⣇⣿⣿⣿⣿ ┃
╰─┤ working: inferred ├────────────────────────────────────────────────┤ 1/5 ├─╯┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸⠀⢸⠀⡇⠀⣿⠀⢸⠀⡇⠀⣿⠀⢸⣿⣿⢸⣿⡇⣿⣿⣿⢸⣿⡇⣿⣿⣿⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢰⠀⡆⠀⣶⠀⢰⠀⡆⠀⣶⠀⢰⣶⣶⢰⣾⡆⣾⣶⣷⢰⣿⡆⣾⣶⣷⣶⣿⣶⣾⣿⣿⣾⣿⣷⣿⣿⣿⣾⣿⣷⣿⣿⣿⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ reported 6  ──▶  review 5  ──▶  integrated 3                                 │┃ ⠀⠀⠀⠀⠀⠀⢀⠀⡀⠀⣀⠀⢀⠀⡀⢀⣀⡀⣸⣀⣇⢀⣿⡀⣸⣀⣇⢀⣿⣀⣸⣿⣿⣸⣿⣇⣿⣿⣿⣸⣿⣇⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                                              │┃ ⠀⠀⠀⠀⠀⠀⢸⠀⡇⠀⣿⠀⢸⠀⡇⢸⣿⡇⣿⣿⣿⢸⣿⡇⣿⣿⣿⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ reports      █████████████▒▒▒     accepted 5  rejected 1                     │┃ ⠀⠀⣤⢠⣤⡄⣼⣤⣧⢠⣿⡄⣼⣤⣧⣼⣿⣧⣿⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ reviews      ██████████▓▓▓▒▒▒     passed 3  started 1  findings 1            │┃ ⣀⣀⣿⣸⣿⣇⣿⣿⣿⣸⣿⣇⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integrations ███████████▓▓▓▓▓     merged 2  running 1                        │┃ oldest unresolved message (age) ─ now 3m41s ─ since dash start               ┃
│                                                                              │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣀⣀⣀ ┃
│ STAGE       STATE    WHO / COMMIT                                        AGE │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣠⣴⣶⣿⣿⣿⣿⣿⣿ ┃
│ integration running  7c1e0f2a                                             5s │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣤⣴⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      started  developer-1 r2                                      40s │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣠⣤⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted designer-1 8bf60fa                                   1m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣤⣴⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted developer-1 9abf487                                  3m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣠⣤⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   5d02b9c4                                             4m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⡄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣠⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣤⣴⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      findings developer-1 r1                                       6m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣠⣶⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⢀⣴⣾⣿⠀⠀⠀⠀⠀⠀⠀⢀⣠⣤⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      passed   designer-1 r1                                        9m │┃ ⠀⠀⠀⠀⠀⠀⠀⣠⣴⣿⣿⣿⣿⡇⠀⠀⠀⠀⠀⢀⣤⣾⣿⣿⣿⣿⠀⠀⣀⣤⣴⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      rejected developer-1 45fe0de                                 11m │┃ ⠀⠀⠀⠀⢀⣴⣾⣿⣿⣿⣿⣿⣿⡇⠀⠀⠀⣠⣶⣿⣿⣿⣿⣿⣿⣿⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted developer-2 fb4a3d5                                 15m │┃ ⣀⣀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣇⣀⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   31aa8e07                                            16m │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/5 ├━┛
│ review      passed   developer-2 r1                                      18m │╭─⁴findings───────────────────────────────────────────────┤ 1 needs operator ├─╮
│ review      passed   developer-0 r1                                      24m ││   ID     TARGET       SEV  STATE     INT REASON                              │
│ report      accepted developer-0 5451671                                 26m ││ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress after 2 interventions   │
│ report      accepted developer-0 3f1d9a0                                 29m ││ ● 0192a4 developer-1  low  open      0/2 scope drift: edits files outside t… │
╰─────────────────────────────────────────────────────────────────────┤ 1/14 ├─╯╰──────────────────────────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  tab focus  1-4 panel  y retry  s skip  c cancel  o observe  p pause  r refresh  ? help  q quit                                                      
```

Colour note: as 1.2.

### 1.4 Header variants (same 100-column strip)

Healthy, one line, no reason row:

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ● HEALTHY                                                  workers ██████░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────╯
```

Degraded while supervision is off. The reason is never silently missing: when the daemon has none, the strip says so. The bottom-border tab shows the supervision switch, which explains a stale value:

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED                                                 workers ██████░░ 3/4   ● answering 1s │
│   reason not recorded (supervision is off, so this value may be stale)                           │
╰──┤ supervision off ├─────────────────────────────────────────────────────────────────────────────╯
```

Controller not answering. The last good frame stays on screen, dimmed (every cell dim, borders dim); the chip is red, the link dot is hollow, and the clock keeps running:

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ○ NO LINK                                                       workers ██████░░ 3/4   ○ 12s ago │
│   controller not answering, retrying in 2s; showing the last good frame from 15:02:11            │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────╯
```

Other header rules:

- The `epoch -/0` text of v1 is removed. The epoch and replacement tabs this spec first proposed (`┤ epoch 4/5 ├`, `┤ replacements 2 ├`) were dropped with `status.supervision`: the header now reads `status.supervisionState` and has no epoch concept.
- The chip text is `● HEALTHY` (green), `● EVALUATING` (yellow), `▲ DEGRADED` (red), `○ NO LINK` (red), `? UNKNOWN` (dim). `PAUSED` replaces the clock tab with `┤ PAUSED ├` (yellow) while polling is paused.
- The strip is a single line when the reason fits in the room between the chip and the worker meter, otherwise it wraps to two lines (at most one extra row, only while degraded or without a link). Below 20 rows the reason is cut with `…` instead of wrapping.
- **Task line.** When the status carries task data, the header box gets one more content line under the strip: `idle` when nothing is active, `working on: plan-19 Dashboard cleanup (1/2)` for one task, the labels joined with ` · ` when several fit, then `N tasks: a, b, +k` (as many names as fit), cut with `…` when even that does not fit. The data is `status.activeTasks` (plans with done/total, and the requirements an agent works on); an older daemon without it falls back to `status.plans` (plans only), and with neither the line is hidden. The line costs one body row.

## 2. Overlays

All three overlays are drawn over the live dashboard, which keeps updating underneath (and is dimmed in the real render). They are shown here at 120x36 over mockup 1.2. An overlay is always centred, always heavy-bordered, and never taller than `H - 2` or wider than `W - 4`.

### 2.1 Help (`?`)

`?`, `Esc` or any other key closes it (as in v1). It replaces v1's full-screen text.

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED  lost contact with developer-2 (pane p4 not responding)         workers █████████░░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 5 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
│   AGENT         ROLE       GEN STATE   ACTIVITY   AGE  Q │┃    SEQ TO           STATE     AGE N DETAIL               ┃
│ ▲ developer-2   Developer  g1  STALLED ░░░░░░░░ 3m41s  2 │┃▌▲  #41 developer-2  unacked 3m41s ● ! no acknowledgemen… ┃
│ ⠋ developer-1   Develop┏━help━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓ · deferred: recipient… ┃
│ ⠋ designer-1    Designe┃                                                                    ┃ ●                      ┃
│ ● pm-1          PM     ┃ NAVIGATE                                                           ┃ ●                      ┃
│ ● supervisor-1  Supervi┃   tab / shift+tab   next / previous panel      1-4   jump to panel ┃ ·                      ┃
│ ○ developer-0   Develop┃   ↑ ↓  or  j k      move the selected row                          ┃──────────────────────… ┃
╰─┤ working: inferred ├──┃                                                                    ┃ed 3m41s ago            ┃
╭─²pipeline─────────────┤┃ ACT   (always asks first; the daemon decides)                      ┃d 14:58:42  notified 1… ┃
│ reported 6  ──▶  review┃   o    observe the selected agent's screen (read-only)             ┃ attempts. Retry sends… ┃
│                        ┃   y    retry the selected message     s   skip     c   cancel      ┃ 6 ─ since dash start   ┃
│ reports      ██████████┃        then press y again to confirm; any other key cancels        ┃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⠀⠀ ┃
│ reviews      ████████▓▓┃                                                                    ┃⠀⢀⠀⡀⠀⣀⠀⢀⠀⡀⠀⣀⠀⢀⠀⣀⢀⣀⡀⣸⣀⣀⣀ ┃
│ integrations █████████▓┃ VIEW                                                               ┃⡄⣼⣤⣧⢠⣿⡄⣼⣤⣧⢠⣿⣤⣼⣤⣿⣼⣿⣧⣿⣿⣿⣿ ┃
│                        ┃   p    pause / resume polling      r   poll now                    ┃⣧⣿⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ STAGE       STATE    WH┃   q    quit  (ctrl+c also quits)                                   ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration running  7c┃                                                                    ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      started  de┃ READING THE SCREEN                                                 ┃ now 3m41s ─ since das… ┃
│ report      accepted de┃   ● idle   ⠋ working (inferred: activity < 30 s or a message in f… ┃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣀⣤⣤⣤⣤⣤ ┃
│ report      accepted de┃   ▲ needs attention: stalled, escalated, or a delivery problem     ┃⠀⠀⠀⠀⠀⠀⠀⣀⣀⣠⣤⣴⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   5d┃   ○ ended   ▌ selected row   ┃ scroll position   n/total in the c… ┃⣠⣤⣤⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      findings de┃                                                                    ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      passed   de┃                                                                    ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      rejected de┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ ? or esc closes ├━┛⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted developer-2 fb4a3d5             15m │┃ ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   31aa8e07                        16m │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/5 ├━┛
│ review      passed   developer-2 r1                  18m │╭─⁴findings───────────────────────────┤ 1 needs operator ├─╮
│ review      passed   developer-0 r1                  24m ││   ID     TARGET       SEV  STATE     INT REASON          │
│ report      accepted developer-0 5451671             26m ││ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress af… │
│ report      accepted developer-0 3f1d9a0             29m ││ ● 0192a4 developer-1  low  open      0/2 scope drift: e… │
╰─────────────────────────────────────────────────┤ 1/14 ├─╯╰──────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  tab focus  1-4 panel  y retry  s skip  c cancel  o observe  p pause  r refresh  ? help  q quit              
```

### 2.2 Retry confirm dialog (`y` on the selected message)

Same semantics as DEC-006: the dialog names the action, the message id, the recipient and the state; a second, separate `y` (not earlier than 300 ms after the dialog opened) confirms; any other key or `Esc` cancels; if the target message's state changes while the dialog is open it closes with the notice `cancelled: the message changed`. The duplicate-delivery warning appears for `sent`/`unacked` retries. The id shown is the one captured when the dialog opened, in full when it fits (this sample shows `#41`, the sequence number; the full id appears in the detail line of the queue panel and the dialog's second line wraps it).

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED  lost contact with developer-2 (pane p4 not responding)         workers █████████░░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 5 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
│   AGENT         ROLE       GEN STATE   ACTIVITY   AGE  Q │┃    SEQ TO           STATE     AGE N DETAIL               ┃
│ ▲ developer-2   Developer  g1  STALLED ░░░░░░░░ 3m41s  2 │┃▌▲  #41 developer-2  unacked 3m41s ● ! no acknowledgemen… ┃
│ ⠋ developer-1   Developer  g2  working ████████    3s  1 │┃ ●  #42 developer-2  queued  3m10s · deferred: recipient… ┃
│ ⠋ designer-1    Designer   g1  working ████████    1s  1 │┃ ●  #44 developer-1  sent       8s ●                      ┃
│ ● pm-1          PM         g1  idle    █████░░░   12s  1 │┃ ●  #45 designer-1   sent       2s ●                      ┃
│ ● supervisor-1  Supervisor g1  idle    ███████░    6s  0 │┃ ●  #46 pm-1         queued     1s ·                      ┃
│ ○ developer-0   Developer  g1  ended              14m  0 │┃ ── selected ───────────────────────────────────────────… ┃
╰─┤ working: inferred ├───────┏━confirm━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ retry ├━┓ queued 3m41s ago            ┃
╭─²pipeline─────────────┤ repo┃                                                          ┃queued 14:58:42  notified 1… ┃
│ reported 6  ──▶  review 5  ─┃ Retry message #41 to developer-2?                        ┃ter 3 attempts. Retry sends… ┃
│                             ┃   state unacked · queued 3m41s ago                       ┃─ max 6 ─ since dash start   ┃
│ reports      ████████████▒▒ ┃                                                          ┃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⠀⠀⠀ ┃
│ reviews      ████████▓▓▓▒▒▒ ┃ ▲ The recipient may already have received it.            ┃⠀⠀⠀⠀⢀⠀⢀⠀⡀⠀⣀⠀⢀⠀⡀⠀⣀⠀⢀⠀⣀⢀⣀⡀⣸⣀⣀⣀ ┃
│ integrations █████████▓▓▓▓▓ ┃   Retrying sends the text again.                         ┃⢠⣤⣤⢠⣼⡄⣼⣤⣧⢠⣿⡄⣼⣤⣧⢠⣿⣤⣼⣤⣿⣼⣿⣧⣿⣿⣿⣿ ┃
│                             ┃                                                          ┃⣼⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣧⣿⣿⣿⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ STAGE       STATE    WHO / C┃   y  confirm and retry        any other key  cancel      ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration running  7c1e0f2┃                                                          ┃⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      started  develop┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛ge) ─ now 3m41s ─ since das… ┃
│ report      accepted designer-1 8bf60fa               1m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣀⣤⣤⣤⣤⣤ ┃
│ report      accepted developer-1 9abf487              3m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣀⣠⣤⣴⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   5d02b9c4                         4m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣠⣤⣤⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      findings developer-1 r1                   6m │┃ ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣀⣤⣤⣴⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ review      passed   designer-1 r1                    9m │┃ ⠀⠀⠀⣀⣤⣶⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣠⣤⣴⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      rejected developer-1 45fe0de             11m │┃ ⣴⣶⣿⣿⣿⣿⣀⣀⣤⣤⣶⣶⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ report      accepted developer-2 fb4a3d5             15m │┃ ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│ integration merged   31aa8e07                        16m │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/5 ├━┛
│ review      passed   developer-2 r1                  18m │╭─⁴findings───────────────────────────┤ 1 needs operator ├─╮
│ review      passed   developer-0 r1                  24m ││   ID     TARGET       SEV  STATE     INT REASON          │
│ report      accepted developer-0 5451671             26m ││ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress af… │
│ report      accepted developer-0 3f1d9a0             29m ││ ● 0192a4 developer-1  low  open      0/2 scope drift: e… │
╰─────────────────────────────────────────────────┤ 1/14 ├─╯╰──────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  tab focus  1-4 panel  y retry  s skip  c cancel  o observe  p pause  r refresh  ? help  q quit              
```

Skip and Cancel use the same dialog with the tab `skip` or `cancel`, no warning block, and the line `y  confirm and skip` / `y  confirm and cancel`.

### 2.3 Observe (`o` on the selected agent)

Read-only. `Esc` or `q` closes it. It shows the last lines the daemon returns (`peek`, 40 lines requested), cleaned with the existing `clean()` rule; the box height decides how many lines are shown, always the newest ones. The title tab carries the Herdr agent status and the standing "unverified text" label (DEC-006 requires the note that pane text is unverified). A transient `peek` failure is shown inside the box in red, not as a footer line.

```text
╭─ cstan dash ─┤ capstan ├┤ run active ├────────────────────────────────────────────────────────┤ 15:02:23 ├┤ - 2s + ├─╮
│ ▲ DEGRADED  lost contact with developer-2 (pane p4 not responding)         workers █████████░░░ 3/4   ● answering 1s │
╰──┤ supervision on ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 5 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━┤ 1 stuck ├┤ input clears 1 ├━┓
│   AGEN┏━observe developer-2━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ agent status: blocked ├┤ unverified text ├━┓       ┃
│ ▲ deve┃                                                                                                      ┃gemen… ┃
│ ⠋ deve┃ developer-2 · claude · pane p4                                                                       ┃pient… ┃
│ ⠋ desi┃                                                                                                      ┃       ┃
│ ● pm-1┃ ● Reading docs/spike-herdr-agents.md                                                                 ┃       ┃
│ ● supe┃ ● Edit(src/dash/theme.ts)                                                                            ┃       ┃
│ ○ deve┃   ⎿ Updated src/dash/theme.ts with 12 additions                                                      ┃─────… ┃
╰─┤ work┃                                                                                                      ┃       ┃
╭─²pipel┃ ✻ Thinking… (3m 41s · ↓ 1.2k tokens · esc to interrupt)                                              ┃ied 1… ┃
│ report┃                                                                                                      ┃sends… ┃
│       ┃   waiting for tool permission:                                                                       ┃tart   ┃
│ report┃   Bash(npm run check)                                                                                ┃⠀⠀⢀⠀⠀⠀ ┃
│ review┃   1. Yes   2. No                                                                                     ┃⣀⡀⣸⣀⣀⣀ ┃
│ integr┃                                                                                                      ┃⣿⣧⣿⣿⣿⣿ ┃
│       ┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ STAGE ┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ integr┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ review┃                                                                                                      ┃e das… ┃
│ report┃                                                                                                      ┃⣀⣤⣤⣤⣤⣤ ┃
│ report┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ integr┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ review┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ review┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ report┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ report┃                                                                                                      ┃⣿⣿⣿⣿⣿⣿ ┃
│ integr┗━┤ esc or q closes ├━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ tail, 12 lines ├━┛ 1/5 ├━┛
│ review      passed   developer-2 r1                  18m │╭─⁴findings───────────────────────────┤ 1 needs operator ├─╮
│ review      passed   developer-0 r1                  24m ││   ID     TARGET       SEV  STATE     INT REASON          │
│ report      accepted developer-0 5451671             26m ││ ▲ 0192a1 developer-2  high ESCALATED 2/2 no progress af… │
│ report      accepted developer-0 3f1d9a0             29m ││ ● 0192a4 developer-1  low  open      0/2 scope drift: e… │
╰─────────────────────────────────────────────────┤ 1/14 ├─╯╰──────────────────────────────────────────────────┤ 1/2 ├─╯
 ↑↓ select  tab focus  1-4 panel  y retry  s skip  c cancel  o observe  p pause  r refresh  ? help  q quit              
```

## 3. Palette and glyphs

### 3.1 Colour roles

Colours are defined once as truecolor hex. Ink passes hex to chalk, which downsamples to 256 or 16 colours by itself according to the terminal's capability; the 256 column below is the value chalk picks, listed so a reviewer can check contrast on a 256-colour terminal. The palette is derived from btop's default theme (greens, olive, violet and rose box colours; green to yellow to red value gradient). It assumes a dark terminal background.

| Role | Truecolor | 256 | 16-colour fallback | Used for |
| --- | --- | --- | --- | --- |
| `fg` | `#cccccc` | 252 | default | Row text |
| `fg.bright` | `#eeeeee` | 255 | bold default | Panel titles, selected row, changed rows |
| `fg.dim` | `#6c6c6c` | 242 | bright black | Column headers' underline rule, empty meter cells, ended agents, the dimmed stale frame, captions. Never used for the only copy of important information |
| `ok` | `#77ca9b` | 115 | green | idle/working, healthy, accepted/passed/merged, meter low end |
| `warn` | `#cbc06c` | 186 | yellow | queued/sent/started/running, evaluating, meter mid, in-progress stage segments |
| `bad` | `#dc4c4c` | 167 | red | degraded, stalled, lost, escalated, unacked/failed/rejected, meter high end, bad stage segments |
| `info` | `#6cb4d8` | 74 | cyan | footer key letters, link dot, spinner, `n/total` counters |
| `select.bg` | `#3b4252` | 238 | reverse video | Selected row background (the row text stays `fg.bright`) |
| `border.header` | `#5a5a5a` | 240 | bright black | Header box |
| `border.agents` | `#556d59` | 65 | green | Agents box |
| `border.agents.focus` | `#77ca9b` | 115 | bright green | Agents box when focused |
| `border.pipeline` | `#5c588d` | 60 | blue | Pipeline box |
| `border.pipeline.focus` | `#8a85c9` | 104 | bright blue | Pipeline box when focused |
| `border.queue` | `#805252` | 95 | red | Queue box |
| `border.queue.focus` | `#d17f7f` | 174 | bright red | Queue box when focused |
| `border.findings` | `#6c6c4b` | 101 | yellow | Findings box |
| `border.findings.focus` | `#cbc06c` | 186 | bright yellow | Findings box when focused |
| `border.work` | `#4b6a7a` | 66 | cyan | v1 work-items box (only when present) |
| `border.work.focus` | `#7fb3cf` | 110 | bright cyan | v1 work-items box when focused |
| `overlay.border` | `#eeeeee` | 255 | bold white | Help, confirm, observe boxes |

The meter and graph gradient has three stops: `ok` at 0 %, `warn` at 60 %, `bad` at 100 %, interpolated per cell in truecolor and snapped to the three 256-colour values in 256-colour mode. A meter fills left to right (workers used / limit); a graph colours each column by its own height.

The selected-row background deliberately differs from btop's reddish selection so it cannot be mistaken for a `bad` state.

Not verified: contrast on light terminal backgrounds. `fg.dim` on a dark background is about 3.4:1, acceptable for secondary text only, which is how it is used.

### 3.2 Glyph set

| Element | Unicode | ASCII fallback |
| --- | --- | --- |
| Box edge (normal) | `╭ ─ ╮ │ ╰ ╯` | `+ - + \| + +` |
| Box edge (focused) | `┏ ━ ┓ ┃ ┗ ┛` | `+ = + \| + +` |
| Tab brackets | `┤ ├` | `[ ]` |
| Hotkey number in the title | `¹ ² ³ ⁴ ⁵` | `1. 2. 3. 4. 5.` |
| Agent working | spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 120 ms | `\| / - \` |
| Agent working, reduced motion | `*` | `*` |
| Idle / fine | `●` | `o` |
| Needs attention (stalled, lost, blocked, escalated, delivery problem) | `▲` | `!` |
| Ended | `○` | `-` |
| Link up / down | `●` / `○` | `*` / `o` |
| Meter filled / empty | `█` / `░` | `#` / `.` |
| Stage bar good / in progress / bad | `█` / `▓` / `▒` | `#` / `+` / `x` |
| Selected row marker | `▌` | `>` |
| Changed row marker (reduced motion) | `+` | `+` |
| Scroll thumb / track | `█` / `│` or `┃` | `#` / `\|` |
| Pipeline flow arrow | `──▶` | `->` |
| Truncation | `…` | `~` |
| Graph (area) | braille `⠀⡀⣀⣄⣤⣦⣶⣷⣿` (2x4 dots per cell) | `_ . - : = #` sparkline, one row |
| Sparkline (one row) | `▁▂▃▄▅▆▇█` | `_ . - : = + * #` |

ASCII mode is selected by `TERM=dumb` (as today) and, new, by a locale that is not UTF-8 (`LC_ALL`/`LC_CTYPE`/`LANG` without `UTF-8`). That second trigger is an addition to `run.ts` (a three-line check) and is listed in section 8.

Width notes: `● ○ ▲ ▌ █ ░ ▒ ▓ ▐ ┃ …` are East Asian Ambiguous. The existing `cellWidth()` in `src/dash/format.ts` counts them as one cell, which is right for Western locales. In a terminal that renders ambiguous characters wide, columns would shift by one cell; the ASCII mode is the remedy and the help text says so.

### 3.3 `NO_COLOR` rendering

With colour off, every colour-carried meaning has a second carrier already present in the mockups above:

| Meaning | Colour | Non-colour carrier |
| --- | --- | --- |
| Focused panel | brighter border | heavy border `┏━┓┃┗┛` |
| Selected row | background | `▌` marker and bold |
| Needs attention | red | `▲` glyph and the words `STALLED`, `LOST`, `BLOCKED`, `ESCALATED`, `DEGRADED`, `!` in the detail text |
| State | green/yellow/red | the state word is always printed (`working`, `unacked`, `ESCALATED`, ...) |
| Meter fill | gradient | `█` versus `░` |
| Stage bar segments | green/yellow/red | `█` / `▓` / `▒` and the count words next to the bar |
| Graph height | gradient | the shape |
| Stale frame (no link) | dimmed | `○ NO LINK` chip and the message text in the strip |

### 3.4 Reduced motion

`--reduced-motion` or `CSTAN_REDUCED_MOTION=1` turns off: the spinner (replaced by a static `*`), the changed-row emphasis (replaced by a `+` marker that stays until the row changes again), and the terminal bell. The clock still ticks each second and graphs still advance one column per poll; both are data, not decoration. No element animates between polls other than the spinner.

## 4. Layout algorithm

Terminal size is `W x H`. Everything below is a pure function of `(W, H, content)` and belongs in `src/dash/layout.ts` (unit-testable without Ink).

### 4.1 Modes

| Condition | Mode |
| --- | --- |
| `W < 60` or `H < 16` | `tiny`: one centred line `terminal too small (need 60x16, have WxH)`; polling continues (unchanged) |
| `60 <= W < 100` | `compact`: one column, panels stacked |
| `W >= 100` | `wide`: two columns, left `floor(W/2)` wide and right `W - floor(W/2)` wide |

Panel order, top to bottom, left to right: compact is agents, pipeline, queue, findings (then work); wide is left = agents, pipeline; right = queue, findings (then work). This keeps DEC-006's split.

### 4.2 Vertical budget

- **Header:** 3 rows (top border with title and clock, content strip, bottom border). The strip wraps to 2 content lines (header 4 rows) only if the reason does not fit and `H >= 20`.
- **Footer:** 1 row: context-sensitive key hints on the left, the transient notice on the right (a notice replaces the hints that no longer fit, it never adds a row). The two footer rows of v1 become one.
- **Body:** `H - header - 1` rows, shared by the panels of each column. In `wide` mode both columns have the full body height and are laid out independently.

### 4.3 Fill, then stretch (replaces `allocate()`)

Each panel reports, from its content:

| Panel | Chrome | Minimum body | Wanted body |
| --- | --- | --- | --- |
| agents | 2 | 1 | 1 header + active agents + ended agents (at most 5) |
| pipeline | 2 | 2 | 1 flow line + 3 stage bars + 2 spacer rows + 1 table header + items (at most 50) |
| queue | 2 | 1 | 1 header + unresolved messages |
| findings | 2 | 1 | 1 header + open and escalated findings |
| work (v1, optional) | 2 | 1 | 1 header + work rows |

Algorithm for one column of `R` rows:

1. Pay every panel's `chrome + minimum body`. If `R` cannot pay that, drop panels in this order until it can: work, findings, pipeline. (At the 60x16 minimum all four fit: `12 body rows = 4 x 3`.)
2. Water-fill toward `wanted body`: repeatedly give one row to the panel with the largest unmet wish, ties broken in the order queue, agents, findings, pipeline. This gives a short panel its full content before a long panel takes more, so a three-agent list is never padded and a long queue is never starved.
3. If rows remain after every wish is met (typical above 30 rows), give them to the stretch elements: first the focused panel, then queue (selected-detail area, then graphs), then agents (graph). A graph never exceeds 24 rows. Rows still left stay as blank interior rows of the bottom-most panel of the column, never as a gap between panels.
4. Inside a panel the rows go to: column header (if body >= 3), data rows, then (queue only) the selected-detail area of 4 rows if at least 5 rows remain after all data rows, then graphs. With 14 or more spare rows the queue shows two graphs (unresolved messages, oldest unresolved age); with fewer, one.
5. Rows beyond the visible window scroll. The cursor row is always visible. The thumb in the right border and the `n/total` tab are the only overflow indicators; the v1 `+N more` text is removed.
6. Attention-first ordering: agents sort `LOST`, `STALLED`, `BLOCKED` first; messages sort delivery problems (in `stuck`, or state `failed`/`expired`/`unacked`) first, then by sequence. Findings sort `escalated` first. Within a group the controller's order is kept. Cursor identity follows the row id, not the index, so a re-sort does not move the selection to a different message.

Worked results for the mockups (rows including borders):

| Size | Header | Agents | Pipeline | Queue | Findings | Footer |
| --- | --- | --- | --- | --- | --- | --- |
| 80x24 | 4 | 6 | 4 | 6 | 3 | 1 |
| 120x36 left / right | 3 | 9 | 23 | 27 | 5 | 1 |
| 160x45 left / right | 3 | 18 | 23 | 36 | 5 | 1 |

### 4.4 Horizontal budget and what collapses first

Let `cw = panel width - 4` (two border cells and one padding cell on each side). Tables have fixed-width columns and one flexible text column that takes the remainder (`AGENT` in agents, `DETAIL` in queue, `REASON` in findings, `WHO / COMMIT` in pipeline); the flexible column truncates with `…` and never wraps. Columns drop at these thresholds:

| `cw` | Drops below it |
| --- | --- |
| 60 | agents `PANE` |
| 56 | queue `N` (notified marker) |
| 46 | agents `ROLE` |
| TASK needs 10 cells | agents `TASK`: dropped first, before `PANE`, once the agent name has its width and fewer than 10 cells are left |
| 40 | agents `ACTIVITY` bar |
| 51 | queue tab `input clears` |
| 45 | pipeline tab `reported > review > integrated` |

Shrinking collapses in this order, first to last:

1. Graph captions shorten; graphs lose rows, then disappear.
2. The queue's selected-detail area.
3. Ended agents.
4. The pipeline's spacer rows, then its items table, then its three stage bars (the pipeline falls back to the flow line plus a one-line history summary, as in 80x24).
5. Column headers (when a panel body is 2 rows or fewer).
6. Optional columns (table above).
7. The header's second line (reason truncated with `…`).
8. Footer hints reduce to `? help  q quit`.
9. Below 60x16: the too-small message.

This replaces `cellsFor()` (which keyed on `50/75/90/100`) in `src/dash/layout.ts`.

### 4.5 Footer hints

Context-sensitive, in btop style (key letter in `info`, label dim): global `tab focus  p pause  r refresh  ? help  q quit` always; plus `↑↓ select` when a table is focused; `y retry  s skip  c cancel` when the queue is focused; `o observe` when agents is focused. The notice (`resolved 0192…: queued`) shows for 5 s on the right.

## 5. Data behind every visual element

"status" means the operator `status` response the dashboard already polls. "ring" means a client-side history ring buffer (one sample per poll, started when the dashboard starts). Nothing below needs a new daemon field except where marked.

| Element | Source | New data? |
| --- | --- | --- |
| Title, project id | `status.projectId` | no |
| Run state tab | `status.run.state` | no |
| Clock | client `Date`, redrawn each second by the existing `useTick(1000)` | no |
| Poll interval tab `- 2s +` | `--interval` value | no (the `-`/`+` keys are a proposal, section 8) |
| Supervision chip | `status.supervisionState` (`enabled`, `supervisor` with `agentId` and `state`, `lastCheck` with `state`, `queuedAt` and `ackedAt`, `openFindings`): the live supervision loop. The chip is `SUPERVISION OFF` when `enabled` is false and the supervisor, the last check and the open findings otherwise | no |
| Legacy health (not drawn) | `status.legacySupervision` is the old `supervision_control` row (`enabled`, `health`, epochs, `replacementAttempts`), which nothing enables any more; `status.legacySupervisionReason` is its degraded reason. The dashboard ignores both; they stay in `status` for old clients | no |
| Worker meter `3/4` | active agents whose kind is not PM or Supervisor (`isWorkerKind`) over `limits.max_workers` from `capstan.toml` (read once client-side) | no. If the config is unreadable the meter is replaced by `workers 3` |
| Link dot, age, `NO LINK` | client poller state (last success time, error code) | no |
| Agents table | `status.agents` (`agentId`, `kind`, `generation`, `state`, `lastActivityAt`) | no |
| `working` glyph and state | inferred: activity within 30 s or a message in `sent`/`unacked` to the agent. Not Herdr's real status | no (still inferred; the box says `working: inferred`) |
| `STALLED`, `LOST` | `status.stalledAgentIds`, `status.lostAgentIds` | no |
| `BLOCKED` | derived: an unresolved message to the agent is in `status.stuck`, or an `escalated` finding targets it | no |
| `TASK` | per agent: a package assignee `plan-19/dash-ui PM-114 <title>`, the plan's Architect `plan-19 · plan`, a requirement worker `<nexora id or ref id> <title>` (from `status.activeTasks`, else the pane's `taskRef` and `taskTitle`), a reviewer with a `started` review the author (or integration) it reviews, otherwise `-`. Labels are cleaned of control characters | no |
| `ACTIVITY` bar | derived from `lastActivityAt`: full bar = activity now, empty = 30 s or more ago. It is recency, not load | no |
| `Q` | count of unresolved `status.messages` to the agent | no |
| `PANE` | `status.panes` | no |
| Working-agents graph | ring of `counts.working` per poll | the ring is new on the client; the model already computes `counts.working`; `app.tsx` records only the unresolved series today (B8) |
| Pipeline flow and stage bars | counts of `status.reports`, `status.reviews`, `status.integrations` by state | see the cap note below |
| Pipeline items | the same three lists, newest first (`createdAt`); who/commit from `agentId`+`commitSha`, `authorAgentId`+`round`, `integrationId` | no |
| 80x24 history summary line | rings of unresolved count and working count | ring only |
| Queue table | `status.messages` (`messageId`, `recipientAgentId`, `state`, `sequence`, `queuedAt`, `deferredReason`, `stateReason`, `lastNotifiedAt`), `status.stuck` for the reason | no |
| Selected-message detail | the same fields, in full | no |
| Unresolved-messages graph | ring of `messages.length` per poll | ring only |
| Oldest-unresolved-age graph | ring of `now - min(queuedAt)` per poll, computed client-side | ring only |
| `input clears N` tab, cleanup/orphan lines | `status.inputClears`, `status.cleanupFailed`, `status.orphanPanes` | no |
| Findings table | `status.agentFindings` filtered to `open` and `escalated` | no |
| Observe text and agent status | the `peek` route (DEC-006 action A) | no new data |
| v1 work box | `status.work` | no |

**Cap note.** The daemon returns at most 20 reports, 20 reviews, 20 integrations and 20 findings (`MAX_STATUS_REPORTS` in `src/commands.ts:258`), newest first, and at most 200 unresolved messages. The stage bars and totals therefore describe the newest 20 per stage, not lifetime totals. When a list reaches 20 the total reads `20+`. The v1 limit of 8 pipeline items (`PIPELINE_ITEMS`, `src/dash/model.ts:5`) should become "as many as fit, up to 20 per stage".

**Data we do not have, so the design does not draw it:**

- btop-style per-process CPU or memory meters. The status route carries no CPU, memory, token or context usage per agent. The activity bar is recency only and is labelled by its column title.
- History from before the dashboard started. The rings start empty (graphs fill from the right) and are captioned `since dash start`, as in DEC-006. Real history needs a new daemon route over `controller_events`, which DEC-006 already defers.
- Health history (a degraded-then-recovered timeline). Only the current health and, while degraded, the current reason exist.
- Herdr's own agent status per poll (DEC-006 keeps `working` inferred; the box says so).
- Throughput (reports per hour). Only the newest 20 items exist, so no rate is shown.
- A reason for `DEGRADED` when the daemon recorded none. The strip says `reason not recorded`.

Ring sizes: 300 samples per series (about 10 minutes at the 2 s default; `SAMPLE_LIMIT` is 30 today at `src/dash/app.tsx:43`, enough for a 15-cell graph but not for the 76-cell graphs above, which hold two samples per cell). Memory is a few kilobytes.

## 6. Ink feasibility

Ink 7's `Box borderStyle` draws a plain frame with no way to put text in the top border, so the design does not use it for panels.

### 6.1 Custom border component (no new dependency)

Draw each panel from strings. A pure module `src/dash/border.ts` (unit-testable, no React):

- `topBorder(width, { number, title, tabs }, glyphs)` returns one string of exactly `width` cells: corner, one rule glyph, superscript number and title, rule fill, the right-aligned tabs each wrapped in the tab brackets, one rule glyph, corner. The fill length is `width - cells(left) - cells(tabs) - 2`; if it would be negative, tabs are dropped from the right, then the title is truncated with `…`.
- `bottomBorder(width, { left, right }, glyphs)` does the same for the bottom edge (left tabs, right tabs such as `1/5`).
- `sideCell(glyphs, focused, thumb)` returns the left or right edge cell; the right one is the scroll thumb when the row is inside the thumb range.
- `glyphs` is chosen from {normal, focused, ASCII, ASCII focused}.

A `Panel` component takes a fixed `width` and `height` and renders `height` lines: the top border, `height - 2` body lines, the bottom border. Each body line is `edge + padding + row + padding + edge`, where `row` has been fitted to `cw` cells by the existing `fit()`/`truncate()` in `src/dash/format.ts` (grapheme-aware, wide characters count two). Because every line has exactly `width` cells, columns line up regardless of content, and the same string functions give golden-file tests of whole panels. Colour is applied per segment with `<Text color=...>`: border glyphs in the box colour, title bold `fg.bright`, tabs `info`/state colours, the selected row wrapped in `<Text backgroundColor={select.bg}>`.

A `Meter` is `█` repeated and `░` repeated, with per-cell colour from the gradient; a `BrailleGraph` is a pure function `(series, cells, rows, max) -> string[]` (each cell is two samples wide and four dots high; the code used to draw the mockups is about 25 lines). Neither needs a library.

Layout is done by `layout.ts` (section 4), not by yoga: panels get explicit sizes. The root is a column of the header lines, a row of two columns (or one), and the footer. That also keeps the three terminal sizes testable as pure data.

### 6.2 Overlays

Ink has no z-index. Two ways, in preference order:

1. **Absolute-positioned `Box`** (`position="absolute"`, `top`, `left`, fixed `width`/`height`) rendered after the dashboard. Every overlay line is padded with spaces to the full overlay width so the cells it covers are overwritten rather than showing through. This relies on Ink drawing an absolute child on top of earlier siblings. **Not verified here:** it needs a one-hour spike in `ink-testing-library` before the implementation is committed to it.
2. **Cell-grid compositor** (fallback and, if the spike disappoints, the primary design). Because every panel is already a list of fixed-width strings, `App` can build the whole screen as `Cell[][]` (character, colour, background, bold, dim), splice the overlay cells over it, and emit one `<Text>` per run of equal style per row. About 150 lines, no dependency, and it makes overlays, the dimmed stale frame and golden-file tests trivial.

Dimming the dashboard behind an overlay is a style flag on every cell of the base grid (route 2) or is skipped (route 1: the overlay's solid border and cleared interior are enough; the live dashboard simply stays at normal brightness).

### 6.3 Other Ink points

- `useWindowSize()` already supplies `W` and `H`; resize recomputes the layout (unchanged).
- Rendering cost: the whole tree re-renders each second (clock) and every 120 ms while an agent is working (spinner). The strings are cheap, but Ink 7 writes full frames, so check for flicker on a slow terminal. If it flickers, enable Ink's incremental/diff rendering if 7.1.1 offers it, or throttle the spinner to the spinner row only. **Not verified.**
- `Text backgroundColor` exists in Ink and gives the selected-row bar. Under `NO_COLOR` it is omitted and the `▌` marker plus bold remain.
- The terminal bell and alternate screen handling in `run.ts` are unchanged.
- **No new dependency.** `ink`, `react` and `ink-testing-library` stay as pinned in DEC-006. `string-width` is not added because `cellWidth()` already exists.

## 7. Confirmed bugs in the current code (v1, branch at `9abf487`)

References are `file:line` in this worktree.

| # | Bug | Evidence |
| --- | --- | --- |
| B1 | **Panels split the height evenly, ignoring content.** `allocate()` gives each panel `floor((rows - panels) / panels)` rows and the remainder to the focused panel. A 3-row agents list gets the same share as a 9-message queue, so the queue prints `+N more` while the agents panel and the bottom of the screen stay empty. This is the "half the screen is empty yet `+2 more`" report | `src/dash/layout.ts:61-76` (the even split is lines 68-73); consumed at `src/dash/app.tsx:432` |
| B2 | **Capacity ignores the extra lines a panel prints.** `windowOf()` is given the panel's row budget, but panels then add lines of their own: ended-agents line, `+more messages not shown`, the sparkline, every cleanup and orphan line, and the pipeline's four summary lines. Total height can exceed the allocation (the outer `Box height={bodyRows}` has no overflow handling), and the `+N more` count is wrong | `src/dash/components/panels.tsx:155` (agents), `:246-258` (queue), `:189-207` (pipeline: body is `capacity - 4` but the summary block is 1 + 3 lines); `src/dash/app.tsx:429` |
| B3 | **No box, no column headers.** Panels are a text title plus text rows; titles are a prefix `>` and underline; focus is only colour and underline. Nothing separates panels | `src/dash/components/panels.tsx:42-69` (`Frame`), rows `:71-96` |
| B4 | **Help replaces the whole body instead of overlaying it.** The observe screen does the same | `src/dash/app.tsx:422-427` (help), `:405-421` (observe); help text `:87-93`; toggling `:270-271` |
| B5 | **The degraded reason can be missing, and when present it is easy to lose.** (a) The reason sits at the end of the single header line after `DEGRADED`, followed by workers; the line is `wrap="truncate-end"`, so at 80 columns the reason is cut first. (b) The reason is only fetched while `health = 'degraded'` and is `null` when the degraded event has no `details.reason`; the model then shows the bare word `DEGRADED`. (c) `health` and `supervision.enabled` are shown side by side as `supervision:off DEGRADED` with no hint that the health value may be stale while supervision is off. **I could not confirm from code which of these produced the user's screenshot** (no status JSON from that run); (a) is certain for any reason longer than the free width | `src/dash/components/panels.tsx:339-347` (one line, truncate); `src/dash/model.ts:345` (reason); `src/controller/core.ts:12960-12973` (`WHERE c.health = 'degraded'`, `typeof row?.reason === "string" ? row.reason : null`); `src/commands.ts:1052` |
| B6 | **The confirm prompt is truncated on narrow terminals.** `confirmText()` is one long sentence containing the full message id (36 characters), recipient, state, the duplicate warning and `Press y again to confirm, any other key cancels.`; it is rendered on a single `wrap="truncate-end"` footer line, so at 80 and even 120 columns the instruction to press `y` can be cut off | `src/dash/actions.ts:47-56`; `src/dash/app.tsx:376-383` and `:446-448` |
| B7 | **Pipeline is plain counts and repeats itself.** The summary line `reported N > review N > integrated N` is followed by three `StageLine`s that restate the same counts, then the items table, with no header | `src/dash/components/panels.tsx:194-208` |
| B8 | **The history graph is incomplete.** DEC-006 specifies two series (unresolved messages and working agents), `Sparkline` exists but is unused, and `app.tsx` records only the unresolved series, only at `wide` width, as one text line inside the queue panel. The ring is 30 samples | `src/dash/components/widgets.tsx:28`; `src/dash/app.tsx:43`, `:108`, `:145-147`; `src/dash/components/panels.tsx:247-252`; `src/dash/layout.ts` (`sparklines: mode === "wide"`) |
| B9 | **Header carries noise and no clock.** `epoch -/0` is printed whenever either epoch is non-null (including the zero default); the line shows `updated 0s ago every 2s` instead of a clock and interval | `src/dash/components/panels.tsx:348-360` |
| B10 | **Footer is a flat list on two rows.** One row for notices and prompts, one dim row of keys regardless of focus; the notice row is reserved even when empty | `src/dash/app.tsx:446-454` |
| B11 | **The stale frame is not dimmed.** DEC-006 says that when the daemon stops answering the last good frame stays visible and dimmed. The code sets the link state and recolours one header line; no panel is dimmed | `src/dash/app.tsx:164-167`, `src/dash/components/panels.tsx:348-353` |
| B12 | **Selection and change markers share one column and colour is the only separation of states.** The row marker is `>` for selected and `+` for changed, drawn inside the truncated text, and the inverse "changed" highlight is only visible with colour | `src/dash/components/panels.tsx:71-96` |

Not bugs, checked: `useTick(1000)` re-rendering every second (`app.tsx:129`) is needed for ages; `trackChanges` change detection works as written.

## 8. Decisions and open questions for the user

Nothing here blocks the mockups; these are the places where the redesign goes beyond DEC-006 or where I could not verify something.

1. **`-` / `+` to change the poll interval.** The top border shows `- 2s +` as in btop. DEC-006 has no such keys. If you do not want new keys, the tab reads `every 2s` instead. (Recommended: keep the keys; they only change the interval within 1 to 60 s.)
2. **Non-UTF-8 locale triggers ASCII mode** (in addition to `TERM=dumb`). Three lines in `run.ts`.
3. **Overlay technique** (section 6.2) needs a spike before implementation starts. The fallback is designed, so this affects effort, not the look.
4. **Interactive tabs.** All tabs in the mockups are passive labels. A `problems only` filter on the queue (a toggle tab, one more key) would fit the btop style but is not in DEC-006; I have not drawn it.
5. **Reason wrapping costs a row.** At 80x24 the wrapped header leaves findings with one row. The alternative (truncate the reason at 80 columns) hides the one piece of text the user said was missing, so the spec wraps.
6. **Ring size 300** and the `20+` cap label change small constants in `model.ts` and `app.tsx`.
7. **Dark background assumed.** The palette is untested on light backgrounds; `NO_COLOR` is the escape.
8. **Not verified:** ambiguous-width glyph behaviour in terminals configured for wide ambiguous characters; Ink 7.1.1 flicker and absolute-overlay behaviour; contrast numbers are computed, not measured on a real terminal. The mockups themselves were generated and width-checked, but have not been seen in a terminal because no implementation exists yet.

## 9. Implementation record

### 9.1 User decisions on section 8

| # | Decision | Built as |
| --- | --- | --- |
| 1 | Yes: `-` / `+` change the poll interval live | `-` shortens and `+` lengthens the interval (the tab reads `- 2s +`, so `-` lowers the number). Steps of 1 s up to 10 s, then 5 s, kept within 1 to 60 (`stepInterval` in `src/dash/poller.ts`). `=` also lengthens, so no shift is needed. The poller restarts its sleep with the new value and does not poll early (`setIntervalMs`) |
| 2 | Accepted: a non-UTF-8 locale triggers ASCII mode | `wantsAscii(env)` in `src/dash/terminal.ts`: `TERM=dumb`, or the first set of `LC_ALL`, `LC_CTYPE`, `LANG` is not UTF-8. With none set the terminal is assumed to be UTF-8 |
| 3 | Do the overlay spike first | Route 1 works; see 9.2 |
| 4 | Yes: a problems-only toggle on the queue | Key `f` (free in DEC-006), a toggle tab `f problems only [ ]` / `[x]` in the queue's top border, listed in help and in the footer when the queue is focused. A selection follows its row id, so toggling keeps the selected message selected; when the filter hides it, the selection falls back to the nearest position and returns to the message when the filter is switched off (`resolveSelection` in `src/dash/app.tsx`) |
| 5 | Accepted: wrap the degraded reason | Built as specified (wraps to a second header line from 20 rows) |
| 6 | Accepted: ring size 300, `20+` cap label | `RING_LIMIT = 300`; stage totals read `20+` when the daemon's list reached its cap of 20 |
| 7 | Dark background assumed; `NO_COLOR` is the escape | Unchanged |

### 9.2 Overlay spike (Ink 7.1.1, `ink-testing-library` 4.0.0)

A `Box position="absolute"` with `marginTop` and `marginLeft`, rendered after a full-screen column of `Text`, is drawn on top of the earlier siblings and overwrites exactly the cells its own lines cover, including the space cells of a padded line. Result for a 3-line heavy box floated at column 8, row 1 over six lines of letters: the letters under the box are gone, the letters beside it are untouched, and the lines above and below are untouched. **Route 1 is the build.** The cell-grid compositor is not needed and was not written. Every overlay line is padded to the box width by `box()` in `src/dash/overlays.ts`, which is what makes the overwrite complete.

A second spike confirmed `Text color="#77ca9b"` downsamples to the 256-colour value in the palette table (`38;5;115`) under `FORCE_COLOR=1`.

### 9.3 Where the build differs from the proposal

- **Screens are data.** `src/dash/view.ts` builds the whole screen as `Line[]` of styled spans (pure, golden-tested); `src/dash/screen.tsx` only paints them. There is no yoga layout for panels. New pure modules: `border.ts`, `graph.ts`, `glyphs.ts`, `lines.ts`, `overlays.ts`; `layout.ts` and `theme.ts` were rewritten; `components/` was removed.
- **Fill, then stretch** is `fillRows()` in `src/dash/layout.ts`. The share of spare rows uses D'Hondt (best `weight / (extra rows + 1)`) with weights agents 4, queue 4, pipeline 2, findings 1, work 1, and every panel's minimum is 3 rows (one body row). The pipeline minimum is therefore 1 body row (the flow line), not 2 as section 4.3 said; at 80x24 it gets 4 rows and shows the flow line plus the one-line history summary. Worked heights from the real code: 80x24: header 4, agents 6, pipeline 4, queue 6, findings 3; 120x36: agents 9, pipeline 23, queue 27 (the proposal said 26), findings 5; 160x45: agents 18, pipeline 23, queue 36, findings 5.
- **The pipeline flow arrow is `──►`**, not `──▶`: `▶` (U+25B6) is an emoji-capable character that the existing `cellWidth()` counts as two cells, which would misalign every row. ASCII fallback `->`.
- **ASCII graphs are `#` column bars** (one dot level per cell, any height), not a one-row sparkline. The compact pipeline summary line uses block sparklines (`▁▂▃▄▅▆▇█`, ASCII `_.-:=+*#`).
- **Graph colour is by row height**, not by column value, so a tall graph shades from the bottom row to the top row.
- **Zero values** draw no dot in an area graph (blank), a non-zero value always lights at least one dot; missing history is blank on the left.
- **Ages** use two units under 10 minutes (`3m41s`) and one unit above (`14m`, `2h05m`, `3d`): `ageDetail()` in `src/dash/format.ts`.
- **No epoch tab.** The epoch tabs were removed together with `status.supervision`; a fresh project printed `epoch -/0` before.
- **`SUPERVISION OFF` chip.** See 9.4.
- **Rendering** uses Ink's `incrementalRendering: true` (`src/dash/run.ts`). See 9.5.
- **Confirm dialog text** is the existing `confirmText()` wrapped to the dialog width, so the DEC-006 sentence and the full message id are shown intact (bug B6).
- **Observe screen:** a failed `peek` shows as a one-line notice in the footer, not inside a box (the box opens only on success).

### 9.4 B5 root cause, confirmed against a live daemon

Historical root cause: a freshly initialised project reported `supervision.enabled: false` and `supervision.health: degraded` with no reason, because `ControllerCore` inserts the `supervision_control` row as `(enabled 0, health 'degraded', target epoch 0)` and never records a degraded event. That row is the old control, which nothing enables any more, so its health said nothing about the live loop. The status now carries `supervisionState` (the configured `enabled`, the supervisor, the last check and the open findings) and the old row as `legacySupervision` with `legacySupervisionReason`; the dashboard shows `○ SUPERVISION OFF` in a neutral colour when `supervisionState.enabled` is false and the live loop's own state otherwise, and never the legacy health or `supervisionReason`.

### 9.5 Flicker

Measured in a real pty (`script`, 120x36, idle project, 8 seconds): full redraw per frame: 110 385 bytes and 362 erase-line sequences; with `incrementalRendering`: 15 599 bytes and 0 erase-line sequences. Every frame is also wrapped in DEC synchronized-output markers (`CSI ? 2026 h`) by Ink, which terminals that support them use to avoid tearing. This shows the redraw volume is small; it does not prove that no terminal flickers. Not tested: terminals without synchronized-output support under a working agent (spinner at 120 ms).

### 9.6 Bugs B1 to B12

| # | Fixed by |
| --- | --- |
| B1 even height split | `fillRows()` in `layout.ts` (content-driven fill, then stretch), unit-tested with budgets of 6 to 80 rows summing exactly |
| B2 capacity ignores extra lines | Each panel is built to an exact height from `queueSections`, `agentSections`, `pipelineSections`; the golden test asserts every frame has exactly `rows` lines of exactly `columns` cells, including for 300-character reasons and 30 long agent ids at 60x16 to 200x60 |
| B3 no boxes or headers | `border.ts`, table headers in `view.ts` |
| B4 help and observe replace the screen | `overlays.ts` + `FloatingBox` (absolute box); the app tests assert the header stays visible under help and under observe |
| B5 degraded reason | 9.4 and the header wrap; golden file `dash-100x30-no-link.txt` and the supervision-off test |
| B6 truncated confirm | Wrapped dialog (`confirmOverlay`) |
| B7 repeated pipeline text | Flow line, stacked stage bars with counts, one items table |
| B8 incomplete history | Rings for unresolved, working and oldest age, 300 samples, sampled on every successful poll; graphs in agents, queue and the compact pipeline summary |
| B9 header noise and no clock | Clock and `- 2s +` tabs, epoch only when meaningful |
| B10 flat footer | One row, hints for the focused panel, notice on the right, hints dropped by priority when narrow |
| B11 stale frame not dimmed | `buildFrame` dims every body cell when the link is down or the status is too large (header and footer stay readable) |
| B12 markers and colour-only state | `▌` selected, `+` changed under reduced motion, heavy border for focus, words and glyphs for every state; golden tests run with colour off |

### 9.7 What was not verified

- **Observe against real Herdr.** The `o` key was exercised in a pty against a daemon without a launcher; it shows the daemon's refusal ("observing agents needs capstan.toml and Herdr") in the footer. The observe box is covered by a golden screen and an app test with a fake `peek`; it has not been seen with real pane text.
- **Colour rendering in a real terminal.** pty captures were taken as plain text (`tmux capture-pane -p`); the colour build is covered by tests on span data and by the 256-colour sequence check, not by eye.
- **A stalled agent, a stuck message and an escalated finding in a live daemon.** These need the delivery driver and a supervisor, which need Herdr. The live captures show a seeded daemon (agents and queued messages) and an empty one; the stalled, stuck and escalated rows are covered by golden screens from status fixtures shaped like the real route.
- **Flicker** beyond the byte counts in 9.5; **ambiguous-width glyphs** in terminals set to wide ambiguous characters; **light backgrounds**.

### 9.4 Corrections after the first real-terminal report

A real-terminal screenshot of the first build showed the defects below. The rendered result is in the `dash-crowded-*` golden files and in `docs/design/cstan-dash-v2-frames.md`. Where this section differs from sections 1 to 8, this section wins.

| Defect | Root cause | Fix |
| --- | --- | --- |
| The agents cursor does not move; the counter reads `1/1` | `rowIds` listed only active agents, so the cursor list had one row | The cursor covers every agent row, active first, then ended. `o` on an ended agent shows a notice and makes no call. A key pressed before the previous one re-rendered started from a stale index and was lost; moves now start from the last selection written |
| `9 ended`, five shown, the wrong five | `buildDashModel` kept `ended.slice(0, 5)` in daemon order and counted the rest in `endedAgentsHidden` | Every ended agent is in the model, newest activity first. The panel scrolls them as one list with the active ones; the thumb and the range or cursor counter show what is hidden |
| Counter `4/13` while unfocused, thumb outside the edge | The counter always showed the cursor, a cursor an unfocused panel does not have. The `▐` thumb is a right-half block, so it draws off the centre of the border cell | A focused panel shows `cursor/total`. An unfocused panel shows the visible range `first-last/total`, and only when rows are hidden. The thumb is `█` |
| Solid green block, grey blocks for empty meters | Bars already use foreground glyphs (no background escape is emitted); three full-block rows stacked with no gap read as one slab, and `░` is a grey hatch in many fonts | The three stage bars have a blank row between them when the pipeline has room for it. Tests assert that no meter or bar span has a background and that only the spec glyphs are used |
| `r1` with no names; `confirm…`, `med…`; pane `w1:…` | Review label was the author only (empty for an integration review); STATE and SEV were sized for the shortest words; PANE was 4 wide | A review reads `reviewer-4 -> developer-1 r1` (an integration review names the integration). STATE is 10 wide (`conflicted`), SEV 8 (`critical`), TARGET and PANE follow the longest value, and the AGENT column drops optional columns before it cuts a name |
| `── selected ──` with no messages | The detail area was reserved whether or not a row existed | Hidden when there is no message; the graphs take the rows. The rule line also no longer ends in `…` |
| Right column loses its right edge | Not reproduced. Every line of every panel and of the header is exactly the terminal width by three measures (cell width, code points, Ink's `string-width`) at every width from 60 to 200; the pty captures at 61 to 160 columns end every line in a border glyph | A property test covers widths 60 to 200 at four heights, both glyph sets, two data sets. If a terminal still loses the last two columns, suspect a terminal narrower than the size Ink reports |
| An escalated finding on a released agent stays `needs operator` | By design in the controller: ending an agent cancels its `open` findings, while `escalated` is a closed state that waits for the operator and is never cancelled | The dashboard dims such a row, shows `target ended`, and counts it as `N target ended` instead of `needs operator` |

The `f problems only` toggle starts off (`useState(false)`); the `[x]` in the screenshot means `f` was pressed while the queue was focused. A test asserts the default.
