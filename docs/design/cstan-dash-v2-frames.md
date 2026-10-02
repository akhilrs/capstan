# `cstan dash` v2: frames captured from a real pty

Captured with tmux (a real pseudo-terminal, `TERM=xterm-256color`, `LANG=en_US.UTF-8`, `capture-pane -p`, plain text, colour not shown) from the built `dist/src/cli.js dash` / `runDash`, on 2026-10-02. Two sources:

- **live**: the built `cstan dash` against a real temp daemon (`cstan init` and `cstan start` in a temp git repo, `CAPSTAN_LAUNCH=off`). A new project has no agents, so these show the empty states and the fresh-project `SUPERVISION OFF` chip.
- **seeded**: the built `runDash` against the in-process test daemon (`test/harness.ts`) with four agents and four queued messages, over the real control socket and the real `status` route. The stalled agent, stuck message and escalated finding of the golden screens need the Herdr-backed delivery driver and are not reproducible here; they are in `test/golden/`.

Regenerate the golden screens (deterministic, from fixtures) with `UPDATE_GOLDEN=1 node --test dist/test/dash-golden.test.js` after a build, then read the diff.

## live empty daemon, 80x24

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├┤ 15:47:47 ├─╮
│ ○ SUPERVISION OFF                                workers ░░░░░░░░ 0/3   ● 0s │
│   health reads degraded until supervision is enabled; this is the starting … │
╰─┤ supervision off ├──────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────────────────────────┤ 0 active ├─╮
│ no agents                                                                    │
│                                                                              │
╰─┤ working: inferred ├────────────────────────────────────────────────────────╯
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮
│ reported 0  ──►  review 0  ──►  integrated 0                                 │
│ reports      ░░░░░░░░░░░░░░░░   0                                            │
│ reviews      ░░░░░░░░░░░░░░░░   0                                            │
│ integrations ░░░░░░░░░░░░░░░░   0                                            │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯
┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
┃ no unresolved messages                                                       ┃
┃                                                                              ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
╭─⁴findings────────────────────────────────────────────────────────────────────╮
│ no open findings                                                             │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  ? help  q quit
```

## live empty daemon, 120x36

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├──────────────────────────────┤ 15:47:51 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ░░░░░░░░░░░░ 0/3   ● answering 0s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 0 active ├─╮┏━³queue━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q │┃     SEQ TO           STATE       AGE N DETAIL            ┃
│ no agents                                                │┃ no unresolved messages                                   ┃
│ working agents (inferred) ─ now 0 ─ since dash start     │┃                                                          ┃
│                                                          │┃ ── selected ───────────────────────────────────────────… ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃ unresolved messages ─ now 0 ─ max 0 ─ since dash start   ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃ oldest unresolved message (age) ─ now 0s ─ since dash s… ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
│                                                          │┃                                                          ┃
╰─┤ working: inferred ├────────────────────────────────────╯┃                                                          ┃
╭─²pipeline─────────────┤ reported > review > integrated ├─╮┃                                                          ┃
│ reported 0  ──►  review 0  ──►  integrated 0             │┃                                                          ┃
│                                                          │┃                                                          ┃
│ reports      ░░░░░░░░░░░░░░   0                          │┃                                                          ┃
│ reviews      ░░░░░░░░░░░░░░   0                          │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## live empty daemon, 160x45

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├──────────────────────────────────────────────────────────────────────┤ 15:47:55 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF  health reads degraded until supervision is enabled; this is the starting value, not a fault     workers ░░░░░░░░░░░░ 0/3   ● answering 0s │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────────────────────────┤ 0 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT                       ROLE       GEN STATE   ACTIVITY    AGE  Q PANE │┃     SEQ TO           STATE       AGE N DETAIL                                ┃
│ no agents                                                                    │┃ no unresolved messages                                                       ┃
│ working agents (inferred) ─ now 0 ─ since dash start                         │┃                                                                              ┃
│                                                                              │┃ ── selected ───────────────────────────────────────────────────────────────… ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃ unresolved messages ─ now 0 ─ max 0 ─ since dash start                       ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃ oldest unresolved message (age) ─ now 0s ─ since dash start                  ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
╰─┤ working: inferred ├────────────────────────────────────────────────────────╯┃                                                                              ┃
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮┃                                                                              ┃
│ reported 0  ──►  review 0  ──►  integrated 0                                 │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│ reports      ░░░░░░░░░░░░░░░░   0                                            │┃                                                                              ┃
│ reviews      ░░░░░░░░░░░░░░░░   0                                            │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
│ integrations ░░░░░░░░░░░░░░░░   0                                            │╭─⁴findings────────────────────────────────────────────────────────────────────╮
│                                                                              ││ no open findings                                                             │
│                                                                              ││                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## seeded daemon, 80x24

```text
╭─ cstan dash ─┤ pa0efd7dfbeb64d508f207c00d279d2b9 ├┤ run active ├┤ 15:47:35 ├─╮
│ ○ SUPERVISION OFF                                workers ████░░░░ 2/4   ● 0s │
│   health reads degraded until supervision is enabled; this is the starting … │
╰─┤ supervision off ├──────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────────────────────────┤ 4 active ├─╮
│   AGENT                       ROLE       GEN STATE   ACTIVITY    AGE  Q PANE │
│ ⠋ developer-agent             Developer  g1  working ████████     3s  1 -    ▐
│ ⠋ developer2-agent            Developer  g1  working ████████     3s  2 -    ▐
│ ⠋ pm-agent                    PM         g1  working ████████     3s  1 -    │
╰─┤ working: inferred ├────────────────────────────────────────────────┤ 1/4 ├─╯
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮
│ reported 0  ──►  review 0  ──►  integrated 0                                 │
│ msgs             ██ 4   working             ██ 4                             │
╰──────────────────────────────────────────────────────────────────────────────╯
┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
┃     SEQ TO           STATE       AGE N DETAIL                                ┃
┃▌●    #1 developer-a… queued       3s ·                                       ▐
┃ ●    #2 developer2-… queued       3s ·                                       ▐
┃ ●    #3 developer2-… queued       3s ·                                       ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛
╭─⁴findings────────────────────────────────────────────────────────────────────╮
│ no open findings                                                             │
╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  ? help  q quit
```

## seeded daemon, 120x36

```text
╭─ cstan dash ─┤ p735bc9cc96404a9f913c8fba30a28f49 ├┤ run active ├──────────────────────────────┤ 15:47:39 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ██████░░░░░░ 2/4   ● answering 0s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 4 active ├─╮┏━³queue━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q │┃     SEQ TO           STATE       AGE N DETAIL            ┃
│ ⠙ developer-a… Developer  g1  working ████████     3s  1 │┃▌●    #1 developer-a… queued       3s ·                   ┃
│ ⠙ developer2-… Developer  g1  working ████████     3s  2 │┃ ●    #2 developer2-… queued       3s ·                   ┃
│ ⠙ pm-agent     PM         g1  working ████████     3s  1 │┃ ●    #3 developer2-… queued       3s ·                   ┃
│ ⠙ supervisor-… Supervisor g1  working ████████     3s  0 │┃ ●    #4 pm-agent     queued       3s ·                   ┃
│ working agents (inferred) ─ now 4 ─ since dash start     │┃ ── selected ───────────────────────────────────────────… ┃
│                                                        ⣿ │┃ #1 to developer-agent  queued  queued 3s ago             ┃
│                                                        ⣿ │┃ message 57dfd02e-fac5-41be-b3d2-3afa639d6619  not notif… ┃
│                                                        ⣿ │┃ state queued   can: skip, cancel                         ┃
│                                                        ⣿ │┃ unresolved messages ─ now 4 ─ max 4 ─ since dash start   ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃                                                        ⣿ ┃
│                                                        ⣿ │┃ oldest unresolved message (age) ─ now 2s ─ since dash s… ┃
│                                                        ⣿ │┃                                                          ┃
│                                                        ⣿ │┃                                                          ┃
╰─┤ working: inferred ├────────────────────────────┤ 1/4 ├─╯┃                                                          ┃
╭─²pipeline─────────────┤ reported > review > integrated ├─╮┃                                                          ┃
│ reported 0  ──►  review 0  ──►  integrated 0             │┃                                                          ┃
│                                                          │┃                                                          ┃
│ reports      ░░░░░░░░░░░░░░   0                          │┃                                                        ⢀ ┃
│ reviews      ░░░░░░░░░░░░░░   0                          │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## seeded daemon, 160x45

```text
╭─ cstan dash ─┤ pf184524cc3a746e6910664f9674cf763 ├┤ run active ├──────────────────────────────────────────────────────────────────────┤ 15:47:43 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF  health reads degraded until supervision is enabled; this is the starting value, not a fault     workers ██████░░░░░░ 2/4   ● answering 0s │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────────────────────────┤ 4 active ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT                       ROLE       GEN STATE   ACTIVITY    AGE  Q PANE │┃     SEQ TO           STATE       AGE N DETAIL                                ┃
│ ⠋ developer-agent             Developer  g1  working ████████     3s  1 -    │┃▌●    #1 developer-a… queued       3s ·                                       ┃
│ ⠋ developer2-agent            Developer  g1  working ████████     3s  2 -    │┃ ●    #2 developer2-… queued       3s ·                                       ┃
│ ⠋ pm-agent                    PM         g1  working ████████     3s  1 -    │┃ ●    #3 developer2-… queued       3s ·                                       ┃
│ ⠋ supervisor-agent            Supervisor g1  working ████████     3s  0 -    │┃ ●    #4 pm-agent     queued       3s ·                                       ┃
│ working agents (inferred) ─ now 4 ─ since dash start                         │┃ ── selected ───────────────────────────────────────────────────────────────… ┃
│                                                                            ⣿ │┃ #1 to developer-agent  queued  queued 3s ago                                 ┃
│                                                                            ⣿ │┃ message cc859e7c-c0a7-4130-a800-a6790660abf0  not notified                   ┃
│                                                                            ⣿ │┃ state queued   can: skip, cancel                                             ┃
│                                                                            ⣿ │┃ unresolved messages ─ now 4 ─ max 4 ─ since dash start                       ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃                                                                            ⣿ ┃
│                                                                            ⣿ │┃ oldest unresolved message (age) ─ now 2s ─ since dash start                  ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
│                                                                            ⣿ │┃                                                                              ┃
╰─┤ working: inferred ├────────────────────────────────────────────────┤ 1/4 ├─╯┃                                                                              ┃
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮┃                                                                              ┃
│ reported 0  ──►  review 0  ──►  integrated 0                                 │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│ reports      ░░░░░░░░░░░░░░░░   0                                            │┃                                                                            ⢠ ┃
│ reviews      ░░░░░░░░░░░░░░░░   0                                            │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛
│ integrations ░░░░░░░░░░░░░░░░   0                                            │╭─⁴findings────────────────────────────────────────────────────────────────────╮
│                                                                              ││ no open findings                                                             │
│                                                                              ││                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## seeded daemon, 120x36: help overlay (`?`)

```text
╭─ cstan dash ─┤ p735bc9cc96404a9f913c8fba30a28f49 ├┤ run active ├──────────────────────────────┤ 15:48:07 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ██████░░░░░░ 2/4   ● answering 0s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 4 active ├─╮┏━³queue━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q │┃     SEQ TO           STATE       AGE N DETAIL            ┃
│ ⠏ developer-a… Developer  g1  working ░░░░░░░░    31s  1 │┃▌●    #1 developer-a… queued      31s ·                   ┃
│ ⠏ developer2-… Developer  g┏━help━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓    31s ·                   ┃
│ ⠏ pm-agent     PM         g┃ NAVIGATE                                                   ┃    31s ·                   ┃
│ ⠏ supervisor-… Supervisor g┃   tab / shift+tab   next / previous panel                  ┃    31s ·                   ┃
│ working agents (inferred) ─┃   1-5               jump to a panel                        ┃──────────────────────────… ┃
│                            ┃   ↑ ↓  or  j k      move the selected row                  ┃  queued 31s ago            ┃
│                            ┃                                                            ┃d2-3afa639d6619  not notif… ┃
│                            ┃ ACT   (always asks first; the daemon decides)              ┃cel                         ┃
│                            ┃   o   observe the selected agent (read-only)               ┃ max 4 ─ since dash start   ┃
│                            ┃   y retry   s skip   c cancel   the selected message       ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃       then press y again to confirm                        ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃   f   queue: show only delivery problems                   ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃                                                            ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃ VIEW                                                       ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃   p   pause / resume polling      r   poll now             ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃   -   +   shorter / longer poll interval (1 to 60 s)       ┃                   ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                            ┃   q   quit (ctrl+c too)       ?   close this help          ┃e) ─ now 31s ─ since dash … ┃
│                            ┃                                                            ┃                            ┃
│                            ┃ READING THE SCREEN                                         ┃                            ┃
╰─┤ working: inferred ├──────┃   ● idle   ⠋ working (inferred)   ▲ needs attention        ┃                            ┃
╭─²pipeline─────────────┤ rep┃   ○ ended   ▌ selected row   ▐ scroll position             ┃                         ⢀⣤ ┃
│ reported 0  ──►  review 0  ┃   working = activity within 30 s or a message in flight    ┃                        ⣰⣿⣿ ┃
│                            ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ ? or esc closes ├━┛                     ⢀⣴⣾⣿⣿⣿ ┃
│ reports      ░░░░░░░░░░░░░░   0                          │┃                                                 ⢀⣴⣿⣿⣿⣿⣿⣿ ┃
│ reviews      ░░░░░░░░░░░░░░   0                          │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## seeded daemon, 120x36: skip confirmation dialog (`s` on the selected message)

```text
╭─ cstan dash ─┤ p735bc9cc96404a9f913c8fba30a28f49 ├┤ run active ├──────────────────────────────┤ 15:48:08 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ██████░░░░░░ 2/4   ● answering 1s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 4 active ├─╮┏━³queue━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q │┃     SEQ TO           STATE       AGE N DETAIL            ┃
│ ⠸ developer-a… Developer  g1  working ░░░░░░░░    32s  1 │┃▌●    #1 developer-a… queued      32s ·                   ┃
│ ⠸ developer2-… Developer  g1  working ░░░░░░░░    32s  2 │┃ ●    #2 developer2-… queued      32s ·                   ┃
│ ⠸ pm-agent     PM         g1  working ░░░░░░░░    32s  1 │┃ ●    #3 developer2-… queued      32s ·                   ┃
│ ⠸ supervisor-… Supervisor g1  working ░░░░░░░░    32s  0 │┃ ●    #4 pm-agent     queued      32s ·                   ┃
│ working agents (inferred) ─ now 4 ─ since dash start     │┃ ── selected ───────────────────────────────────────────… ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃ #1 to developer-agent  queued  queued 32s ago            ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃ message 57dfd02e-fac5-41be-b3d2-3afa639d6619  not notif… ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃ state queued   can: skip, cancel                         ┃
│                           ┏━confirm━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ skip ├━┓max 4 ─ since dash start   ┃
│                           ┃                                                              ┃                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                           ┃ Skip message 57dfd02e-fac5-41be-b3d2-3afa639d6619 to         ┃                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                           ┃ developer-agent (state queued)? Press y again to confirm,    ┃                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                           ┃ any other key cancels.                                       ┃                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                           ┃                                                              ┃                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                           ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛                  ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃ oldest unresolved message (age) ─ now 31s ─ since dash … ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
│                                                 ⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
╰─┤ working: inferred ├────────────────────────────┤ 1/4 ├─╯┃                                                          ┃
╭─²pipeline─────────────┤ reported > review > integrated ├─╮┃                                                       ⢀⣤ ┃
│ reported 0  ──►  review 0  ──►  integrated 0             │┃                                                      ⣰⣿⣿ ┃
│                                                          │┃                                                   ⢀⣴⣾⣿⣿⣿ ┃
│ reports      ░░░░░░░░░░░░░░   0                          │┃                                                 ⢀⣴⣿⣿⣿⣿⣿⣿ ┃
│ reviews      ░░░░░░░░░░░░░░   0                          │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

## seeded daemon, 120x36: problems-only filter on (`f`), nothing is stuck

```text
╭─ cstan dash ─┤ p735bc9cc96404a9f913c8fba30a28f49 ├┤ run active ├──────────────────────────────┤ 15:48:10 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ██████░░░░░░ 2/4   ● answering 1s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────────────────┤ 4 active ├─╮┏━³queue━━━━━━━━━━━━┤ no problems ├┤ f problems only [x] ├━┓
│   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q │┃     SEQ TO           STATE       AGE N DETAIL            ┃
│ ⠦ developer-a… Developer  g1  working ░░░░░░░░    34s  1 │┃ no delivery problems                                     ┃
│ ⠦ developer2-… Developer  g1  working ░░░░░░░░    34s  2 │┃                                                          ┃
│ ⠦ pm-agent     PM         g1  working ░░░░░░░░    34s  1 │┃ ── selected ───────────────────────────────────────────… ┃
│ ⠦ supervisor-… Supervisor g1  working ░░░░░░░░    34s  0 │┃                                                          ┃
│ working agents (inferred) ─ now 4 ─ since dash start     │┃                                                          ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃ unresolved messages ─ now 4 ─ max 4 ─ since dash start   ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃ oldest unresolved message (age) ─ now 33s ─ since dash … ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
│                                                ⢸⣿⣿⣿⣿⣿⣿⣿⣿ │┃                                                          ┃
╰─┤ working: inferred ├────────────────────────────┤ 1/4 ├─╯┃                                                          ┃
╭─²pipeline─────────────┤ reported > review > integrated ├─╮┃                                                       ⢀⣾ ┃
│ reported 0  ──►  review 0  ──►  integrated 0             │┃                                                      ⣴⣿⣿ ┃
│                                                          │┃                                                    ⢠⣾⣿⣿⣿ ┃
│ reports      ░░░░░░░░░░░░░░   0                          │┃                                                  ⢀⣴⣿⣿⣿⣿⣿ ┃
│ reviews      ░░░░░░░░░░░░░░   0                          │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  p pause  r refresh  ? help  q quit             cancelled
```

## seeded daemon, 120x36: focus moved with Tab to findings (the heavy border follows focus)

```text
╭─ cstan dash ─┤ p735bc9cc96404a9f913c8fba30a28f49 ├┤ run active ├──────────────────────────────┤ 15:48:13 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                          workers ██████░░░░░░ 2/4   ● answering 1s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                        │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────╯
┏━¹agents━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 4 active ├━┓╭─³queue────────────┤ no problems ├┤ f problems only [ ] ├─╮
┃   AGENT        ROLE       GEN STATE   ACTIVITY    AGE  Q ┃│     SEQ TO           STATE       AGE N DETAIL            │
┃▌⠴ developer-a… Developer  g1  working ░░░░░░░░    36s  1 ┃│ ●    #1 developer-a… queued      36s ·                   │
┃ ⠴ developer2-… Developer  g1  working ░░░░░░░░    36s  2 ┃│ ●    #2 developer2-… queued      36s ·                   │
┃ ⠴ pm-agent     PM         g1  working ░░░░░░░░    36s  1 ┃│ ●    #3 developer2-… queued      36s ·                   │
┃ ⠴ supervisor-… Supervisor g1  working ░░░░░░░░    36s  0 ┃│ ●    #4 pm-agent     queued      36s ·                   │
┃ working agents (inferred) ─ now 4 ─ since dash start     ┃│ ── selected ───────────────────────────────────────────… │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│ #1 to developer-agent  queued  queued 36s ago            │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│ message 57dfd02e-fac5-41be-b3d2-3afa639d6619  not notif… │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│ state queued   can: skip, cancel                         │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│ unresolved messages ─ now 4 ─ max 4 ─ since dash start   │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│ oldest unresolved message (age) ─ now 35s ─ since dash … │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                          │
┃                                                ⣿⣿⣿⣿⣿⣿⣿⣿⣿ ┃│                                                          │
┗━┤ working: inferred ├━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1/4 ├━┛│                                                          │
╭─²pipeline─────────────┤ reported > review > integrated ├─╮│                                                      ⢀⣤⣾ │
│ reported 0  ──►  review 0  ──►  integrated 0             ││                                                     ⣰⣿⣿⣿ │
│                                                          ││                                                  ⢀⣴⣾⣿⣿⣿⣿ │
│ reports      ░░░░░░░░░░░░░░   0                          ││                                                ⢀⣴⣿⣿⣿⣿⣿⣿⣿ │
│ reviews      ░░░░░░░░░░░░░░   0                          │╰──────────────────────────────────────────────────┤ 1/4 ├─╯
│ integrations ░░░░░░░░░░░░░░   0                          │╭─⁴findings────────────────────────────────────────────────╮
│                                                          ││ no open findings                                         │
│                                                          ││                                                          │
╰──────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────╯
 ↑↓ select  o observe  tab focus  -/+ interval  p pause  r refresh  ? help  q quit                           cancelled
```

## seeded daemon with 1 active and 9 ended agents (defect-fix round)

Captured with tmux the same way as above (`TERM=xterm-256color`, `LANG=en_US.UTF-8`, `capture-pane -p`), on 2026-10-02, from the built `runDash` against a **seeded stand-in daemon**: a small script that answers the control socket's `status` route with a fixed operator status. It is not the real controller (the real status route needs Herdr for stalled, stuck and delivery state). The seed: one active agent (`pm-1`), nine ended agents (`supervisor-1`, `designer-1`, `developer-1`, `reviewer-1` to `reviewer-6`), three reports, six reviews by named reviewers (three of reports, three of integrations), three confirmed integrations, no messages, supervision off, and one escalated finding of severity `medium` on the ended `designer-1`. The same data is the `crowded()` fixture in `test/dash-fixtures.ts` and the `dash-crowded-*` golden files. The earlier "seeded daemon" sections above predate this round and show the first v2 build (`▐` thumb, five ended agents at most).

Each size is shown at start (queue focused, as the dashboard starts) and after pressing `1` then `j` three times: the agents cursor moves through the ended rows (`4/10`, on `reviewer-4`). Seven more widths (61, 99, 100, 101, 117, 119, 133 columns, 30 rows) were captured and every line of each ends in a border glyph.

### 80x24, at start

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├┤ 16:53:16 ├─╮
│ ○ SUPERVISION OFF                                workers ░░░░░░░░ 0/3   ● 0s │
│   health reads degraded until supervision is enabled; this is the starting … │
╰─┤ supervision off ├──────────────────────────────────────────────────────────╯
╭─¹agents──────────────────────────────────────────────┤ 1 active ├┤ 9 ended ├─╮
│   AGENT                      ROLE       GEN STATE   ACTIVITY    AGE  Q PANE  │
│ * pm-1                       PM         g1  working ███████░     5s  0 w1:p1 █
│ ○ reviewer-6                 Reviewer   g1  ended             5m03s  0 -     █
│ ○ reviewer-5                 Reviewer   g1  ended               10m  0 -     │
│ ○ reviewer-4                 Reviewer   g1  ended               15m  0 -     │
╰─┤ working: inferred ├─────────────────────────────────────────────┤ 1-4/10 ├─╯
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮
│ reported 3  ──►  review 6  ──►  integrated 3                                 │
│ reports      ███████████▒▒▒▒▒   3  accepted 2  rejected 1                    │
│ reviews      █████████████▒▒▒   6  passed 5  findings 1                      │
╰──────────────────────────────────────────────────────────────────────────────╯
┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
┃ no unresolved messages                                                       ┃
┃                                                                              ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
╭─⁴findings─────────────────────────────────────────────────┤ 1 target ended ├─╮
│ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended                      │
╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  ? help  q quit
```

### 80x24, after 1 j j j

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├┤ 16:53:22 ├─╮
│ ○ SUPERVISION OFF                                workers ░░░░░░░░ 0/3   ● 0s │
│   health reads degraded until supervision is enabled; this is the starting … │
╰─┤ supervision off ├──────────────────────────────────────────────────────────╯
┏━¹agents━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1 active ├┤ 9 ended ├━┓
┃   AGENT                      ROLE       GEN STATE   ACTIVITY    AGE  Q PANE  ┃
┃ * pm-1                       PM         g1  working ███████░     7s  0 w1:p1 █
┃ ○ reviewer-6                 Reviewer   g1  ended             5m05s  0 -     █
┃ ○ reviewer-5                 Reviewer   g1  ended               10m  0 -     ┃
┃▌○ reviewer-4                 Reviewer   g1  ended               15m  0 -     ┃
┗━┤ working: inferred ├━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 4/10 ├━┛
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮
│ reported 3  ──►  review 6  ──►  integrated 3                                 │
│ reports      ███████████▒▒▒▒▒   3  accepted 2  rejected 1                    │
│ reviews      █████████████▒▒▒   6  passed 5  findings 1                      │
╰──────────────────────────────────────────────────────────────────────────────╯
╭─³queue────────────────────────────────┤ no problems ├┤ f problems only [ ] ├─╮
│ no unresolved messages                                                       │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯
╭─⁴findings─────────────────────────────────────────────────┤ 1 target ended ├─╮
│ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended                      │
╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  o observe  tab focus  p pause  r refresh  ? help  q quit
```

### 118x34, at start

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├────────────────────────────┤ 16:53:25 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                            workers ░░░░░░░░ 0/3   ● answering 0s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                      │
╰─┤ supervision off ├────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents─────────────────────────┤ 1 active ├┤ 9 ended ├─╮┏━³queue━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT                ROLE       GEN STATE      AGE  Q │┃     SEQ TO           STATE       AGE DETAIL             ┃
│ * pm-1                 PM         g1  working     5s  0 │┃ no unresolved messages                                  ┃
│ ○ reviewer-6           Reviewer   g1  ended    5m03s  0 │┃ unresolved messages ─ now 0 ─ max 0 ─ since dash start  ┃
│ ○ reviewer-5           Reviewer   g1  ended      10m  0 │┃                                                         ┃
│ ○ reviewer-4           Reviewer   g1  ended      15m  0 │┃                                                         ┃
│ ○ reviewer-3           Reviewer   g1  ended      25m  0 │┃                                                         ┃
│ ○ reviewer-2           Reviewer   g1  ended      33m  0 │┃                                                         ┃
│ ○ reviewer-1           Reviewer   g1  ended      41m  0 │┃                                                         ┃
│ ○ developer-1          Developer  g1  ended      50m  0 │┃                                                         ┃
│ ○ designer-1           Designer   g1  ended      58m  0 │┃                                                         ┃
│ ○ supervisor-1         Supervisor g1  ended    1h06m  0 │┃                                                         ┃
╰─┤ working: inferred ├───────────────────────────────────╯┃                                                         ┃
╭─²pipeline────────────┤ reported > review > integrated ├─╮┃ oldest unresolved message (age) ─ now 0s ─ since dash … ┃
│ reported 3  ──►  review 6  ──►  integrated 3            │┃                                                         ┃
│ reports      █████████▒▒▒▒   3  accepted 2  rejected 1  │┃                                                         ┃
│                                                         │┃                                                         ┃
│ reviews      ███████████▒▒   6  passed 5  findings 1    │┃                                                         ┃
│                                                         │┃                                                         ┃
│ integrations █████████████   3  confirmed 3             │┃                                                         ┃
│ STAGE       STATE      WHO / COMMIT                 AGE │┃                                                         ┃
│ integration confirmed  i3012345                     10m █┃                                                         ┃
│ review      passed     reviewer-6 -> i3 r1          13m █┃                                                         ┃
│ integration confirmed  i2012345                     13m █┃                                                         ┃
│ integration confirmed  i1012345                     16m █┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
│ review      passed     reviewer-5 -> i2 r1          18m │╭─⁴findings────────────────────────────┤ 1 target ended ├─╮
│ review      passed     reviewer-4 -> i1 r1          23m ││ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended │
│ review      findings   reviewer-3 -> developer-…    28m ││                                                         │
╰──────────────────────────────────────────────┤ 1-7/12 ├─╯╰─────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

### 118x34, after 1 j j j

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├────────────────────────────┤ 16:53:31 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF                                                            workers ░░░░░░░░ 0/3   ● answering 0s │
│   health reads degraded until supervision is enabled; this is the starting value, not a fault                      │
╰─┤ supervision off ├────────────────────────────────────────────────────────────────────────────────────────────────╯
┏━¹agents━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1 active ├┤ 9 ended ├━┓╭─³queue───────────┤ no problems ├┤ f problems only [ ] ├─╮
┃   AGENT                ROLE       GEN STATE      AGE  Q ┃│     SEQ TO           STATE       AGE DETAIL             │
┃ * pm-1                 PM         g1  working     7s  0 ┃│ no unresolved messages                                  │
┃ ○ reviewer-6           Reviewer   g1  ended    5m05s  0 ┃│ unresolved messages ─ now 0 ─ max 0 ─ since dash start  │
┃ ○ reviewer-5           Reviewer   g1  ended      10m  0 ┃│                                                         │
┃▌○ reviewer-4           Reviewer   g1  ended      15m  0 ┃│                                                         │
┃ ○ reviewer-3           Reviewer   g1  ended      25m  0 ┃│                                                         │
┃ ○ reviewer-2           Reviewer   g1  ended      33m  0 ┃│                                                         │
┃ ○ reviewer-1           Reviewer   g1  ended      41m  0 ┃│                                                         │
┃ ○ developer-1          Developer  g1  ended      50m  0 ┃│                                                         │
┃ ○ designer-1           Designer   g1  ended      58m  0 ┃│                                                         │
┃ ○ supervisor-1         Supervisor g1  ended    1h06m  0 ┃│                                                         │
┗━┤ working: inferred ├━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 4/10 ├━┛│                                                         │
╭─²pipeline────────────┤ reported > review > integrated ├─╮│ oldest unresolved message (age) ─ now 0s ─ since dash … │
│ reported 3  ──►  review 6  ──►  integrated 3            ││                                                         │
│ reports      █████████▒▒▒▒   3  accepted 2  rejected 1  ││                                                         │
│                                                         ││                                                         │
│ reviews      ███████████▒▒   6  passed 5  findings 1    ││                                                         │
│                                                         ││                                                         │
│ integrations █████████████   3  confirmed 3             ││                                                         │
│ STAGE       STATE      WHO / COMMIT                 AGE ││                                                         │
│ integration confirmed  i3012345                     10m █│                                                         │
│ review      passed     reviewer-6 -> i3 r1          13m █│                                                         │
│ integration confirmed  i2012345                     13m █│                                                         │
│ integration confirmed  i1012345                     16m █╰─────────────────────────────────────────────────────────╯
│ review      passed     reviewer-5 -> i2 r1          18m │╭─⁴findings────────────────────────────┤ 1 target ended ├─╮
│ review      passed     reviewer-4 -> i1 r1          23m ││ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended │
│ review      findings   reviewer-3 -> developer-…    28m ││                                                         │
╰──────────────────────────────────────────────┤ 1-7/12 ├─╯╰─────────────────────────────────────────────────────────╯
 ↑↓ select  o observe  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

### 160x45, at start

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├──────────────────────────────────────────────────────────────────────┤ 16:53:35 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF  health reads degraded until supervision is enabled; this is the starting value, not a fault     workers ░░░░░░░░░░░░ 0/3   ● answering 0s │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭─¹agents──────────────────────────────────────────────┤ 1 active ├┤ 9 ended ├─╮┏━³queue━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ no problems ├┤ f problems only [ ] ├━┓
│   AGENT                      ROLE       GEN STATE   ACTIVITY    AGE  Q PANE  │┃     SEQ TO           STATE       AGE N DETAIL                                ┃
│ * pm-1                       PM         g1  working ███████░     5s  0 w1:p1 │┃ no unresolved messages                                                       ┃
│ ○ reviewer-6                 Reviewer   g1  ended             5m03s  0 -     │┃ unresolved messages ─ now 0 ─ max 0 ─ since dash start                       ┃
│ ○ reviewer-5                 Reviewer   g1  ended               10m  0 -     │┃                                                                              ┃
│ ○ reviewer-4                 Reviewer   g1  ended               15m  0 -     │┃                                                                              ┃
│ ○ reviewer-3                 Reviewer   g1  ended               25m  0 -     │┃                                                                              ┃
│ ○ reviewer-2                 Reviewer   g1  ended               33m  0 -     │┃                                                                              ┃
│ ○ reviewer-1                 Reviewer   g1  ended               41m  0 -     │┃                                                                              ┃
│ ○ developer-1                Developer  g1  ended               50m  0 -     │┃                                                                              ┃
│ ○ designer-1                 Designer   g1  ended               58m  0 -     │┃                                                                              ┃
│ ○ supervisor-1               Supervisor g1  ended             1h06m  0 -     │┃                                                                              ┃
│ working agents (inferred) ─ now 1 ─ since dash start                         │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│                                                                            ⢸ │┃                                                                              ┃
│                                                                            ⢸ │┃                                                                              ┃
╰─┤ working: inferred ├────────────────────────────────────────────────────────╯┃ oldest unresolved message (age) ─ now 0s ─ since dash start                  ┃
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮┃                                                                              ┃
│ reported 3  ──►  review 6  ──►  integrated 3                                 │┃                                                                              ┃
│ reports      ███████████▒▒▒▒▒   3  accepted 2  rejected 1                    │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│ reviews      █████████████▒▒▒   6  passed 5  findings 1                      │┃                                                                              ┃
│                                                                              │┃                                                                              ┃
│ integrations ████████████████   3  confirmed 3                               │┃                                                                              ┃
│ STAGE       STATE      WHO / COMMIT                                      AGE │┃                                                                              ┃
│ integration confirmed  i3012345                                          10m │┃                                                                              ┃
│ review      passed     reviewer-6 -> i3 r1                               13m │┃                                                                              ┃
│ integration confirmed  i2012345                                          13m │┃                                                                              ┃
│ integration confirmed  i1012345                                          16m │┃                                                                              ┃
│ review      passed     reviewer-5 -> i2 r1                               18m │┃                                                                              ┃
│ review      passed     reviewer-4 -> i1 r1                               23m │┃                                                                              ┃
│ review      findings   reviewer-3 -> developer-1 r1                      28m │┃                                                                              ┃
│ review      passed     reviewer-2 -> developer-1 r1                      33m │┃                                                                              ┃
│ review      passed     reviewer-1 -> designer-1 r1                       38m │┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
│ report      rejected   developer-1 45fe0de                               46m │╭─⁴findings─────────────────────────────────────────────────┤ 1 target ended ├─╮
│ report      accepted   developer-1 9abf487                               48m ││ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended                      │
│ report      accepted   designer-1 8bf60fa                                56m ││                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  y retry  s skip  c cancel  f problems  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```

### 160x45, after 1 j j j

```text
╭─ cstan dash ─┤ pe7275bc45ede472f8acaa75ce193ee32 ├┤ run active ├──────────────────────────────────────────────────────────────────────┤ 16:53:41 ├┤ - 2s + ├─╮
│ ○ SUPERVISION OFF  health reads degraded until supervision is enabled; this is the starting value, not a fault     workers ░░░░░░░░░░░░ 0/3   ● answering 0s │
╰─┤ supervision off ├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
┏━¹agents━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 1 active ├┤ 9 ended ├━┓╭─³queue────────────────────────────────┤ no problems ├┤ f problems only [ ] ├─╮
┃   AGENT                      ROLE       GEN STATE   ACTIVITY    AGE  Q PANE  ┃│     SEQ TO           STATE       AGE N DETAIL                                │
┃ * pm-1                       PM         g1  working ███████░     7s  0 w1:p1 ┃│ no unresolved messages                                                       │
┃ ○ reviewer-6                 Reviewer   g1  ended             5m05s  0 -     ┃│ unresolved messages ─ now 0 ─ max 0 ─ since dash start                       │
┃ ○ reviewer-5                 Reviewer   g1  ended               10m  0 -     ┃│                                                                              │
┃▌○ reviewer-4                 Reviewer   g1  ended               15m  0 -     ┃│                                                                              │
┃ ○ reviewer-3                 Reviewer   g1  ended               25m  0 -     ┃│                                                                              │
┃ ○ reviewer-2                 Reviewer   g1  ended               33m  0 -     ┃│                                                                              │
┃ ○ reviewer-1                 Reviewer   g1  ended               41m  0 -     ┃│                                                                              │
┃ ○ developer-1                Developer  g1  ended               50m  0 -     ┃│                                                                              │
┃ ○ designer-1                 Designer   g1  ended               58m  0 -     ┃│                                                                              │
┃ ○ supervisor-1               Supervisor g1  ended             1h06m  0 -     ┃│                                                                              │
┃ working agents (inferred) ─ now 1 ─ since dash start                         ┃│                                                                              │
┃                                                                              ┃│                                                                              │
┃                                                                              ┃│                                                                              │
┃                                                                              ┃│                                                                              │
┃                                                                              ┃│                                                                              │
┃                                                                            ⣿ ┃│                                                                              │
┃                                                                            ⣿ ┃│                                                                              │
┗━┤ working: inferred ├━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┤ 4/10 ├━┛│ oldest unresolved message (age) ─ now 0s ─ since dash start                  │
╭─²pipeline─────────────────────────────────┤ reported > review > integrated ├─╮│                                                                              │
│ reported 3  ──►  review 6  ──►  integrated 3                                 ││                                                                              │
│ reports      ███████████▒▒▒▒▒   3  accepted 2  rejected 1                    ││                                                                              │
│                                                                              ││                                                                              │
│ reviews      █████████████▒▒▒   6  passed 5  findings 1                      ││                                                                              │
│                                                                              ││                                                                              │
│ integrations ████████████████   3  confirmed 3                               ││                                                                              │
│ STAGE       STATE      WHO / COMMIT                                      AGE ││                                                                              │
│ integration confirmed  i3012345                                          10m ││                                                                              │
│ review      passed     reviewer-6 -> i3 r1                               13m ││                                                                              │
│ integration confirmed  i2012345                                          13m ││                                                                              │
│ integration confirmed  i1012345                                          16m ││                                                                              │
│ review      passed     reviewer-5 -> i2 r1                               18m ││                                                                              │
│ review      passed     reviewer-4 -> i1 r1                               23m ││                                                                              │
│ review      findings   reviewer-3 -> developer-1 r1                      28m ││                                                                              │
│ review      passed     reviewer-2 -> developer-1 r1                      33m ││                                                                              │
│ review      passed     reviewer-1 -> designer-1 r1                       38m │╰──────────────────────────────────────────────────────────────────────────────╯
│ report      rejected   developer-1 45fe0de                               46m │╭─⁴findings─────────────────────────────────────────────────┤ 1 target ended ├─╮
│ report      accepted   developer-1 9abf487                               48m ││ ▲ 0192a1 designer-1 medium   ESCALATED 2/2 target ended                      │
│ report      accepted   designer-1 8bf60fa                                56m ││                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓ select  o observe  tab focus  -/+ interval  p pause  r refresh  ? help  q quit
```
