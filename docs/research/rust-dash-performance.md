# Rust dashboard (`cstan-dash`): CPU, memory and redraw cost, measured

Date: 2026-10-06. Machine: the workstation behind `dash-performance.md` (8 cores, Linux 6.1.0-48-amd64, Node 24.6.0).
`cstan-dash` is the `npm run build:dash` release build (`lto = true`, `codegen-units = 1`, `strip = true`), version 0.1.1.

This document has two parts. **After PM-131 (`f6606ad`)** is the current code. Everything below it, from "Before PM-131", is the
**pre-fix baseline**: it was measured on `f897095` plus the dash-e2e checks of `9c9fcce` (branch
`test/PM-129-end-to-end-parity-and-performance-checks`), when the measurement itself changed no product code. The baseline is kept
because it shows what the fix removed.

CPU is percent of **one core** from `/proc/<pid>/stat` (utime + stime) over the window; RSS is `VmRSS` from `/proc/<pid>/status`.

## After PM-131 (`f6606ad`)

`f6606ad` keeps the previous frame and its buffer: a spinner tick repaints only the lines that differ (a full paint on the first
frame, a resize or an overlay change) and reuses the cached history rings when they did not change. Same harness and machine as
below (`docs/research/dash-bench/dash-bench.py`, 160x45 pty, fake daemon, release build), 3 x 60 s per case, one at a time.

| Target | Measured (median of 3 x 60 s) | Verdict |
|---|---|---|
| CPU <= 1.5% idle | **0.7%** (0.7, 0.7, 0.7) | pass |
| CPU <= 3.5% with 4 agents working | **2.8%** (2.8, 2.8, 2.8) | **pass** (was 3.6%) |
| RSS <= 30 MB | **7 to 8 MB** | pass |

Redraw cost per spinner tick (160x45, `tasks` fixture, 4 agents working), re-measured on `f6606ad` in a release build with
`dash/tests/e2e_redraw.rs` (crossterm backend into a byte counter):

* **Bytes:** a tick writes **85 bytes** and changes 4 cells (one spinner glyph per working agent); a tick that also steps a cell of
  a recency bar writes 139 bytes and 5 cells. The first paint writes 14,057 bytes. These are the same as before the fix: the
  fix reduces the work to produce the bytes, not the bytes.
* **Time:** `redraw_if_needed` for one spinner tick (view rebuild, painting the changed lines, ratatui's diff, escape-sequence
  write into the counter) took **1.27 ms** on average over 2,000 ticks. This is a different cut than the baseline's per-phase
  table (0.85 + 1.24 = about 2.1 ms for `build_frame` plus a whole-screen paint and diff, measured on a `TestBackend`), so the two
  are not a like-for-like speed-up factor; only the 3.6% to 2.8% CPU figures are directly comparable.
* **Not re-measured after the fix:** the 10-minute RSS growth runs, the thread split and the Node comparison. Their figures below are
  from the baseline code; the RSS ranges after the fix (7 to 8 MB) are from the 60 s runs only, and the Node side does not change.

Verdict: both CPU targets and the RSS target are met on this machine. The miss reported in the baseline below is fixed by `f6606ad`.

# Before PM-131: baseline on `f897095` plus `9c9fcce`

## Baseline result against the targets

| Target (plan-22/dash-e2e) | Measured (median of 3 x 60 s) | Verdict |
|---|---|---|
| CPU <= 1.5% idle | **0.8%** (0.8, 0.8, 0.8) | pass |
| CPU <= 3.5% with 4 agents working | **3.6%** (3.6, 3.6, 3.7) | **MISSED by 0.1 points** (fixed by `f6606ad`, see above) |
| RSS <= 30 MB | **6.0 to 7.5 MB** (idle 6.0, 6.1, 6.6; working 7.3, 7.5, 7.5) | pass |
| RSS growth < 1 MB, minute 2 to 10 of a 10-minute run | **+8 kB idle, +44 kB with 4 working** | pass |

A missed target is not a pass. At this baseline the working-agents CPU was over the target; the profile below is what led to the
PM-131 fix (`f6606ad`), which brought it to 2.8%. The three runs are tightly grouped (3.6, 3.6, 3.7), so this is not noise
at the 0.1-point level on this machine; it is a real, small miss. The 10-minute run gave the same 3.7% (whole run, 600 s).

## Node dashboard beside it (same harness, same fake daemon)

`cstan dash` with `CSTAN_DASH=node`, started as `node dist/src/cli.js dash` (React/Ink in development mode, which is what a source
or npm install runs; see `dash-performance.md` section 1 for why).

| Case | Rust CPU % (median) | Node CPU % (median) | Rust RSS end (MB) | Node RSS end (MB) | tty bytes/s Rust | tty bytes/s Node |
|---|---|---|---|---|---|---|
| 1 dash, 2 s, nothing working | 0.8 | 21.6 (21.4, 21.7, 21.6) | 6.0 to 6.6 | 377 to 392 | about 380 | about 1730 |
| 1 dash, 2 s, 4 agents working | 3.6 | 94.0 (93.4, 94.5, 94.0) | 7.3 to 7.5 | 661 to 708 | about 1150 | about 19500 |

Rust is 27 times cheaper idle and 26 times cheaper with 4 agents working; about 55 to 95 times less memory. The Node figures match
`dash-performance.md` (22.9% idle and 95.3% working in development mode). The packaged binary's Node dash runs in production mode
(8.9 to 10.2% idle, 58% working, 290 to 380 MB there); I did not re-measure that mode here.

## Method

Everything ran in scratch directories under `/tmp/cstb/` (short paths: a unix socket path has a length limit). No live project,
daemon, ledger or pane was used, `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` were unset, and the only processes signalled were the ones the
scripts started.

* **Scratch project.** `cstan init` in a fresh git repo; `docs/research/dash-bench/fake-daemon.mjs` listens on the project's own
  `.capstan/state/control.sock`, so `cstan dash` finds a running "daemon" (it answers `ping` with the status result, pid 0) and
  starts none of its own.
* **Status.** The 209,000-byte `status` captured from the 450-agent scratch ledger for `dash-performance.md` (not committed; the
  capture lives only in that run's scratch directory). `fake-daemon.mjs` marks the first N agents just active on every request, so N = 4
  shows 4 agents working and N = 0 shows none.
* **Dashboard.** `docs/research/dash-bench/dash-bench.py --dashes 1 --seconds 60` runs `node dist/src/cli.js dash` on a 160x45 pty.
  For Rust, `CSTAN_DASH=rust CSTAN_DASH_BIN=<cstan-dash>` makes `cstan dash` `execve` the Rust binary; the pid the harness samples is
  the pid that `cstan dash` was started with (the sampler for the long runs confirmed the process is `cstan-dash`).
* **Matrix.** Rust idle x3, Rust 4 working x3, Node idle x3, Node 4 working x3, each 60 s, one at a time. The Node 4-working runs
  were repeated with a fresh fake daemon for each run: `fake-daemon.mjs` has no error handler and exits on `ECONNRESET` when a
  client is killed mid-request, and the first matrix attempt lost the fake after its first Node run (the later "Node working" runs then
  talked to a real scratch daemon that `cstan dash` auto-started and read 11%; those two figures are discarded, and that scratch
  daemon was stopped with `cstan stop` in its own scratch project).
* **10-minute runs.** One Rust dash idle and one with 4 working, side by side (separate projects and daemons), 600 s each. A second
  sampler read `VmRSS` every 10 s (dash-bench rounds RSS to whole MB, too coarse for a 1 MB growth bound) and the per-thread CPU ticks:

| 10-minute run | CPU (whole run) | RSS min 2 / min 10 | growth | fitted slope, min 2 to 10 | range min 2 to 10 |
|---|---|---|---|---|---|
| idle | 0.8% | 7,020 / 7,028 kB | +8 kB | 15 kB/min | 6,240 to 7,152 kB |
| 4 working | 3.7% | 7,680 / 7,724 kB | +44 kB | 41 kB/min | 6,796 to 7,816 kB |

RSS breathes by about 1 MB (allocator and the frame buffer) but does not grow: the growth over 8 minutes is under 0.05 MB.

## Profile of the baseline 3.6% (4 agents working)

No `perf`, `strace` or `valgrind` is installed here and `perf_event_paranoid` is 3, so this is a thread split plus a per-phase timing,
not a sampling profile.

* **Where the CPU is.** Over the 600 s run (60,000 ticks of one core): main thread 2,013 ticks = 3.36%; the second thread (the poll
  thread: socket read, JSON parse, model build; or the input thread) 171 ticks = 0.29%. The idle run: main 312 ticks = 0.52%, second
  168 ticks = 0.28%. So polling and parsing a 209 kB status is the same 0.28% idle or working; **the extra 2.8 points are the main
  thread redrawing**.
* **What redraws.** With 4 agents working the spinner ticks every 120 ms (8.3 draws per second) plus the 1 Hz clock. In a release build
  (160x45, `tasks` fixture, `cargo test --release`, 2,000 iterations each):

| Phase per redraw | Time |
|---|---|
| `build_frame` (the whole 45-row frame, rebuilt) | 0.85 ms |
| `paint_screen` into the buffer plus ratatui's diff (`TestBackend`) | 1.24 ms |
| Sum | about 2.1 ms |

  2.1 ms x 9.3 draws per second = 1.95% of a core; the rest of the main thread's 3.36% is the `ViewState` clone and comparison in
  `redraw_if_needed`, crossterm's escape-sequence writes and the `write` syscalls to the pty (about 1,150 bytes per second).
* **What a redraw writes** (`dash/tests/e2e_redraw.rs`, crossterm backend into a byte counter, 160x45, `tasks` fixture): the first
  paint writes 14,057 bytes. One spinner tick writes **85 bytes** and changes exactly 4 cells (one spinner glyph per working
  agent); a tick that also steps a cell of an agent's 8-cell recency bar (30 s window) writes 139 bytes and 5 cells. 100 unchanged
  polls cause no draw and no byte. The 200-byte bound holds; the output volume is not the cost, the rebuild is.
* **Where a fix could look** (the PM-131 fix took (1) and (2)), in the order of what a spinner tick wastes: (1) a tick rebuilds the whole frame
  (`build_frame`, 0.85 ms) and repaints all 7,200 cells (`paint_line` sets the symbol and style of every cell, 1.24 ms) to change
  4 cells; a tick-only path that updates just the spinner cells, or reuses the previous frame when only `tick` changed, would remove
  most of the 2 ms. (2) `view_state(..., with_rings = false)` is built and compared on every wake, and `with_rings = true` clones the
  three history rings for the paint. (3) The 8.3 Hz wake rate itself (`SPINNER_MS = 120`); a slower spinner is a product choice.
  Even removing about 40% of the redraw cost would put the 4-working case under 3.5%.

## Redraw and parity checks run alongside (baseline)

* `dash/tests/e2e_parity.rs`: for every fixture, a fake daemon serves `fixture.status` on a unix socket, the real `Client`, `Poller`,
  `AppState::on_poll` (`build_dash_model`), `build_frame` and `paint_screen` fill a `TestBackend`; every cell's text, foreground,
  background and modifiers equal the Node frame at every fixture size (more than 100,000 cells compared).
* `dash/tests/e2e_redraw.rs`: the 85/139-byte tick and the 100 unchanged polls above.
* `docs/research/dash-bench/rust-keys.py` and its transcript `docs/research/rust-dash-keys.txt`: every key at 80x24 and 160x45,
  77 of 77 steps each, against `fake-daemon.mjs` behind a call-logging proxy.

## What I could not measure

* Only Linux x86-64, one machine, one terminal type (a pty read by the harness); the terminal emulator's own CPU is not counted.
* The status is a captured synthetic ledger (450 agents, 200 messages); a real project's status differs in size.
* The packaged binary's Node dash (production mode) was not re-run; its numbers are from `dash-performance.md`.
* No sampling profiler is available, so the profile is a thread split plus per-phase timings, not a flame graph.
* Other agents were using the machine during the runs (the 8 cores were not idle); the grouping of the three runs suggests the
  effect is small, but a quiet machine could shift each figure by a few tenths.
