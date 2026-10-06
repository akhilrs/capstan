# Why `cstan dash` uses so much CPU and memory

Measurement only; no product code was changed. Date: 2026-10-06. Machine: 8 cores, Linux 6.1, Node 24.6.0.
Branch `chore/PM-119-profile-dash-and-daemon-cpu-and-memory`, built from `main` at `9105c75`.

CPU figures are percent of **one core** (100% = one core fully busy), taken from `/proc/<pid>/stat` over the stated window.
RSS is from `/proc/<pid>/status`.

## Summary

1. **The dash is run in React development mode.** `dist/src/cli.js` is started with plain `node`, `NODE_ENV` is unset, so React and Ink load their `*.development.js` builds. Only the packaged binary defines `NODE_ENV=production` (`scripts/build-binary.mjs:183`). In dev mode the dash costs 2.3x the CPU at idle and **leaks without bound** (142 MB to 1018 MB RSS in 10 minutes, about 1.3 MB/s). Production mode: plateau near 300 MB. This is the biggest and cheapest fix.
2. **Any working agent makes the dash redraw the whole screen 8 times a second** (`SPINNER_MS = 120`, `app.tsx:51,169`). One dash with 4 working agents: 95% CPU in dev mode, 58% in production mode. With `--reduced-motion` (no spinner): 22.5% and 10.7%. This, not polling, is what produces "more than 100% with two dashboards".
3. **The daemon spends about 160 ms of CPU on every `status` request** on a 450-agent ledger, so each dashboard polling every 2 s costs the daemon about 8 percentage points. Two thirds of it is two avoidable queries (a correlated-subquery scan of `seats` x `actors`, about 63 ms, and an N+1 re-prepared `SELECT * FROM agents` per agent in `listAgents`, about 25 ms). The daemon driver tick calls the same `listAgents` every 2 s, which is most of the daemon's 6.2% idle CPU.
4. Polling, JSON parsing and model building in the dash are **not** a problem (about 5 ms per poll, under 0.3% CPU).
5. A Rust dashboard would fix 1, 2 and the dash's 140 MB baseline, but **not 3**. Items 1 and 3 are small, local Node changes; they should be done first.

## Method

Everything ran in scratch directories under `/tmp/cstprof/` (the socket path of a project under the scratchpad was too long for a unix socket). No live project, daemon, ledger or pane was used for any number below.

* **Scratch project and daemon.** `cstan init` in a fresh git repo, daemon started from this worktree's `dist` with `herdr` removed from `PATH` and `CAPSTAN_*`/`HERDR_*` unset. Ledger seeded through `ControllerCore` (the same API the tests use): 450 developer agents (449 later ended by the daemon at start because they have no pane; only the PM stays active), 452 seats/actors, 3000 messages (200 unresolved, the status cap), 300 reports, SQLite file 34 MB.
* **Dashboards.** `docs/research/dash-bench/dash-bench.py` starts N real `cstan dash` processes, each on its own pty sized 160x45, samples CPU/RSS of each dash and of the daemon, and counts bytes written to the tty.
* **Active agents.** The real daemon ends agents that have no herdr pane, so "agents working" could not be produced on the real daemon. `fake-daemon.mjs` replays a captured real `status` response (209,000 bytes) over a unix socket and refreshes `lastActivityAt` on 4 agents so the dash model shows them as working (`WORKING_WINDOW_MS = 30 s`). `dash-run.mjs` runs the same `runDash` UI against it.
* **Profiles.** `node --cpu-prof` on a dash and on the daemon; summarised with `cpuprofile-summary.mjs` and `cpuprofile-inclusive.mjs`.
* **Per-request cost.** `status-probe.mjs` makes the same `status` call as the dash poller and times it; `query-timing.mjs` times the `statusSnapshot` SQL on a copy of the scratch ledger.
* **Memory.** 10-minute run of one dash; `perf-entries.mjs` preload reports `performance.getEntries().length` and heap every 5 s.

The scratch ledger is synthetic. It has no plans, findings, reviews or integrations (see "What I could not measure").

## 1. Which process is busy (60 s windows, default 2 s refresh unless noted)

Dash running as `node dist/src/cli.js dash`, i.e. **development mode** (what a source or npm install does):

| Case | dash CPU % (each) | dash RSS end (MB) | daemon CPU % | daemon RSS (MB) |
|---|---|---|---|---|
| 0 dashes (daemon alone) | n/a | n/a | **6.2** | 113 |
| 1 dash, 2 s, no agent working | 22.9 | 425 | 14.0 | 139 |
| 2 dashes, 2 s | 22.7 / 23.6 | 411 / 419 | 22.0 | 176 |
| 1 dash, `--reduced-motion`, 2 s | 23.2 | 412 | 13.9 | 192 |
| 1 dash, 10 s (`--interval 10`) | 17.8 | 382 | 7.8 | 192 |
| 1 dash, 30 s | 17.5 | 378 | 6.6 | 192 |
| 2 dashes, 10 s | 17.7 / 17.7 | 379 / 378 | 9.3 | 192 |
| 1 dash, 4 agents working (fake daemon) | **95.3** | 726 | n/a | n/a |
| 1 dash, 4 working, `--reduced-motion` | 22.5 | 376 | n/a | n/a |

Same, **production mode** (`NODE_ENV=production`, what the packaged binary does):

| Case | dash CPU % (each) | dash RSS end (MB) | daemon CPU % | daemon RSS (MB) |
|---|---|---|---|---|
| 1 dash, 2 s, no agent working (240 s run) | 8.9 | 296 | 13.5 | 299 |
| 2 dashes, 2 s | 10.2 / 10.1 | 331 / 329 | 21.4 | 299 |
| 1 dash, 2 s (second run, with write counter) | 10.2 | 327 | 13.6 | 299 |
| 1 dash, 4 agents working (fake daemon) | **58.0** | 379 | n/a | n/a |
| 1 dash, 4 working, `--reduced-motion` | 10.7 | 288 | n/a | n/a |

Reading the tables:

* The dash is the heavier process (23% / 95% dev, 10% / 58% production), not the daemon. The daemon adds 8 points per dashboard at 2 s.
* Slowing the poll from 2 s to 30 s barely moves the dash (22.9 to 17.5%): polling is not what the dash spends CPU on. It cuts the daemon from 14.0% to 6.6%, i.e. back to its idle level.
* Daemon cost per `status` request, from the differences above: (14.0 - 6.2) / 0.5 req/s = 156 ms; (22.0 - 6.2) / 1.0 = 158 ms; (7.8 - 6.2) / 0.1 = 160 ms. Linear in the number of dashboards and in 1/interval.
* Two dashboards in production mode with nothing working are about 20% dash CPU plus 21% daemon; with agents working they reach the user's ">100%" easily (2 x 58 in production, 2 x 95 in dev).

Not comparable between runs: the tty bytes per second. Early dev runs wrote 17 KB/s, later runs 1.4 to 3.9 KB/s with similar CPU (about 1 MB in the first minute, much less afterwards). I did not find out why; I do not rely on it.

## 2. Inside the dash

Per poll (`status-probe.mjs`, 209 KB response): `JSON.stringify` + `JSON.parse` 1.8 ms, `buildDashModel` 1.5 ms, plus the `sha256(JSON.stringify(status))` change check (not timed separately, same order as stringify). About **4 to 5 ms per poll, under 0.3% CPU at 2 s**. Polling, parsing and model building are noise.

The cost is in rendering. `App` re-renders on:

* a 1 s `useTick` (`app.tsx:166`), always; the clock text changes every second so every one of these produces a different frame;
* a 120 ms `useTick` whenever `model.counts.working > 0` and motion is not reduced (`app.tsx:169`);
* every poll, even when the status hash is unchanged: `setRings(...)` (`app.tsx:199`) always creates a new object, so React re-renders on every poll. `setLink("ok")` alone would not.

Each render runs `buildFrame` and then Ink lays out and diffs a full React tree of `<Text>` lines. Estimated cost per frame: (58.0 - 10.7) points over (8.3 - 1) extra frames/s, about **65 ms per frame** in production mode. The idle 1 Hz case is therefore about 6.5 points of the 10% plus GC and baseline.

Production-mode CPU profile (1 dash, 2 s, nothing working, 60 s; busy 9.5 s of 64.6 s wall incl. profiler overhead):

| Where | Share of busy time |
|---|---|
| `writeUtf8String` (tty write of the frame, see caveat) | 34.6% |
| `src/dash/format.ts` `cellWidth` | 8.2% (8.8% for the whole file) |
| GC | 5.2% |
| `@alcalzone/ansi-tokenize` tokenize/diff/styledChars/ansiCodes (Ink output) | about 13% |
| `string-width` + emoji regexp | about 4.3% + 1.3% |
| `react-reconciler` production | 4.3% |
| Ink `output.js`, `reconciler.js`, `squash-text-nodes` | about 4.5% |
| `src/dash/view.ts` (`buildFrame`) | 1.9% |
| model building, JSON, hashing | under 1% |

Caveat on the tty write: in the harness 198 `process.stdout.write` calls (98 KB) took 1.9 s of wall time inside `write` (about 10 ms each), which is slow for ~500 bytes. The pty line discipline, or my reader, may inflate this; a real terminal emulator will have different cost, and the emulator's own CPU is not counted here. Treat 35% as an upper bound for the write share. The rest (about 6 s of 9.5 s) is Ink/React/format work and is real.

Development-mode profile (same case, 16.8 s busy): React `createElement`/`jsx` development builds 12.2% + 7.7%, `react-reconciler.development` 10.5%, `createTask`, and `performance.measure` (usertiming) about 2.6% on top of the same Ink work. That is the dev-mode overhead behind 23% vs 10%.

Does it redraw when nothing changed? The status hash prevents a model rebuild (`app.tsx:180`), but the 1 s tick and the `setRings` call still re-render the whole tree every second and every poll. Ink's `incrementalRendering: true` keeps the tty output small (about 1.5 KB/s) but the frame is still built, laid out and diffed each time.

## 3. Inside the daemon

`status` response: **209,000 bytes** on the scratch ledger. By top-level key: `agents` 100,990 (451 records, 450 of them ended), `roles` 59,534 (452 seat rows), `messages` 42,623 (200), `reports` 4,741, everything else under 200 bytes each. About 76% of the payload is two lists of mostly ended agents and seats.

Wall latency of one request (`status-probe.mjs`, 100 requests, idle daemon): p50 174 ms, min 157 ms, max 318 ms (30-request run: p50 160 ms). With 2 dashboards the daemon is single-threaded, so requests queue; at 2 dashboards x 0.5 req/s it is 16% busy on this alone.

Daemon CPU profile (one process, 34 s idle + 100 status requests; busy 21 s):

| Where (inclusive, 100 requests) | ms total | ms per request |
|---|---|---|
| `status` handler (all) | 16,263 | 163 |
| `listAgents` (451 x `agentRecord`) | 7,717 | 77 (25 in isolation, see below) |
| `statusSnapshot` | 6,995 | 70 |
| `supervisionActivity` | 884 | 9 |
| `unresolvedMessages` | 645 | 6 |
| `activeTasks` | 67 | 0.7 |
| `pipelineCounts` | 56 | 0.6 |
| `agentPanes`, `integrations`, `pmMailSummary` | under 50 each | under 0.5 |
| driver `#tick` (about 27 ticks) | 2,236 | about 80 per tick |

85.5% of the daemon's busy time is in `src/controller/sqlite.ts` (`all` 41%, `get` 25%, `prepare` 16%): query execution and re-preparing the statement on every call.

Per-query timings on a copy of the scratch ledger (`query-timing.mjs`, millisecond per query):

| Query | Time | Why |
|---|---|---|
| `roles` in `statusSnapshot` (`status.ts:340`): seats + two correlated subqueries | **63.4 ms** | plan: `SEARCH a USING INDEX sqlite_autoindex_actors_3 (project_id=?)`; the index only has `project_id`, so each of 452 seats scans all actors/assignments of the project: seats x actors |
| same query after `CREATE INDEX actors(project_id, seat_id, active)` and `assignments(project_id, seat_id, authority_state, created_at)` on a copy | **1.4 ms** | |
| `listAgents` as N+1 `SELECT * FROM agents WHERE ... agent_id = ?`, preparing each time | 25.0 ms | 452 prepares |
| same with one cached prepared statement | 8.9 ms | |
| one `SELECT * FROM agents WHERE project_id = ?` | 1.2 ms | |

The `roles` query grows with seats x actors, and every spawned worker adds a seat and an actor. The user's live ledger had stateVersion 19,869 and 400+ agents (per the PM), so it is likely larger than the scratch one. The numbers will be worse there, not better.

Scaling with dashboards: linear. Each additional dashboard adds about 156 ms of daemon CPU per poll (8 points at 2 s, 1.6 at 10 s, 0.5 at 30 s). The daemon also writes one JSON log line per request (about 190 bytes), so one dashboard at 2 s adds about 8 MB/day to `daemon.log`; the live `daemon.log` was 46 MB on 2026-10-06 (observed with `ls`).

Idle daemon (no dashboard): 6.2% CPU. The driver tick (every 2 s) calls `listAgents` too (same N+1 over all agents, ended ones included), about 80 ms per tick on this ledger, i.e. about 4 of the 6 points. The real daemon also calls herdr per active agent; that was not present here.

## 4. Memory

Dash RSS, one dash, 2 s, nothing working, 10 minutes, development mode:

RSS in MB at each successive sample (the harness collects one sample per wake-up, so the 21 points span the 600 s, roughly every 30 s):

142, 306, 381, 411, 437, 462, 507, 551, 609, 609, 634, 636, 702, 754, 755, 837, 848, 851, 959, 1017, 1017.

Growth is roughly linear, about **1.3 MB/s, to 1018 MB**, with no plateau.

Cause, confirmed by a counter: in development mode React records `performance.measure` entries for every render and Node keeps them. Entries counted by `performance.getEntries().length` rose from 157 at 5 s to 5,905 at 80 s (about 77 per second, never freed). In production mode: 0 entries. I did not take a heap snapshot, so "this alone explains all of the growth" is not proven; the correlation and the production run (below) support it.

Production mode, same case: 138 MB at start, 283 MB at 1 min, then 289 to 296 MB flat for the next 3 minutes (240 s run). Heap used fluctuates between 64 and 137 MB while RSS stays near 300 MB. So even production mode costs about 300 MB RSS; about 140 MB of it is the Node + React + Ink baseline at start.

Daemon RSS: 113 MB at start, 139 to 192 MB after serving dashboards, flat at 193 to 196 MB over the 10-minute run, then 298 MB later in the session (after the 240 s production run and two further dash runs). No leak pattern in the first 10 minutes; the step to 298 MB is unexplained (likely heap growth with GC not returning pages; not investigated).

## Root causes, ranked by impact

| # | Cause | Evidence | Impact |
|---|---|---|---|
| 1 | Dash runs React/Ink in development mode when started from `dist` (no `NODE_ENV=production`) | profile shows `react.development.js`, `createTask`, `performance.measure`; production run: 23% to 10% idle, 95% to 58% busy; 5,905 perf entries vs 0 | 2.3x CPU at idle, 1.6x when working, **unbounded memory growth (1 GB in 10 min)** |
| 2 | Whole-app re-render every 120 ms while any agent is working | 58% (prod) / 95% (dev) vs 10.7% / 22.5% with `--reduced-motion`; about 65 ms per frame | up to 48 points of CPU per dashboard; this is what exceeds 100% with two dashboards |
| 3 | `statusSnapshot` `roles` query is seats x actors (missing composite index) and `listAgents` is N+1 with a new prepare per row | 63 ms + 25 to 77 ms of 160 ms per request; 1.4 ms with an index | 8 points of daemon CPU per dashboard at 2 s; about 4 points of the daemon's idle 6.2% (driver tick); grows with the ledger |
| 4 | Every render is expensive even when the screen barely changes (65 ms/frame, tty write, `cellWidth`, ansi tokenising) and there is a forced 1 Hz render plus a render on every poll (`setRings`) | profile; 10% idle in production | 10 points idle per dashboard |
| 5 | Status payload includes all ended agents and seats (209 KB, 76% in two lists) | key sizes | small CPU (parse + hash about 4 ms), mostly memory churn and socket bytes; dash RSS baseline |
| 6 | Node + React + Ink baseline plus GC churn | 138 MB at start, 290 to 330 MB steady in production | memory only |
| 7 | One log line per request | 46 MB daemon.log | disk growth, minor CPU |

## Fix options and estimated gains

Estimates marked "est." are extrapolated from the measurements above, not measured.

| Option | What | Estimated gain | Cost / risk |
|---|---|---|---|
| A. Run the dash in production mode | set `process.env.NODE_ENV ??= "production"` at the top of `runDash` (or in `cli.ts` before importing `react`/`ink`); the SEA binary already does | measured: 23% to 10% idle, 95% to 58% busy; memory growth stops (plateau about 300 MB instead of 1 GB in 10 min) | one line; React dev warnings disappear for developers (keep an env override) |
| B. Make rendering cheap: isolate the spinner and clock from the main tree, cache `buildFrame` output per panel, stop `setRings` re-rendering on unchanged polls (`setRings` bail-out) | the 8 Hz spinner only needs to change a few cells | est.: busy case from 58% to about 12 to 15% (frame cost 65 ms to under 10 ms for a spinner-only update); idle 10% to about 4 to 6% | moderate; touches `app.tsx`, `view.ts`; goldens exist |
| C. Redraw only on change | skip the React render when the new frame's lines are identical to the last (cheap string compare), and throttle the spinner to about 4 fps | est. 30 to 50% of the remaining idle cost; spinner at 4 fps halves the busy cost | small; can ride on B |
| D. Slower idle polling | back off from 2 s to 10 s after a few polls with an unchanged hash and no key press; return to 2 s on change or key | measured daemon effect: +7.8 points per dashboard at 2 s, +1.6 at 10 s, +0.4 at 30 s; dash CPU almost unchanged | small; delayed view of changes unless C below exists |
| E. Fix the two daemon queries | add `actors(project_id, seat_id, active)` and `assignments(project_id, seat_id, authority_state, created_at)` indexes (new migration) and make `listAgents` one query; cache prepared statements in `Statement`/`Database.prepare` | measured pieces: `roles` 63 ms to 1.4 ms, `listAgents` 25 ms to about 1.2 to 9 ms; est. per request 160 ms to about 20 to 30 ms (about 85% off); the same fix removes about 4 of the daemon's 6.2 idle points | needs a migration and ledger tests; the biggest daemon-side win |
| F. Smaller status | drop or window ended agents and seats (only active plus the last N ended), or add a separate "full history" call | payload 209 KB to about 30 KB (est., -85%); dash parse/hash time 4 ms to under 1 ms; lower dash memory churn; daemon CPU only slightly lower because the cost is in queries, not serialisation | wire contract change; the dash `model.ts` and tests depend on the current shape |
| G. Delta / "if-changed" status | dash sends the last `stateVersion` (or a hash); the daemon returns `{unchanged:true}` after one cheap `SELECT state_version` | idle daemon cost per poll 160 ms to about 1 ms (est.) when nothing changed; the status also contains time-dependent parts (stalled, stuck, ages) so those need either a cheap tail or separate calls | moderate; careful with fields that change without a ledger write |
| H. Push/subscribe status feed | the daemon keeps a long-lived socket per dashboard and sends a status (or delta) when `stateVersion` or driver state changes | removes polling load entirely: daemon cost goes from `dashboards x 0.5/s x 160 ms` to `changes x 160 ms` (est. under 1 point when idle; with E also cheap when busy); latency of updates drops to the tick interval | largest change: the daemon protocol is one request, one reply, connection closed (`client.ts`, `daemon.ts`); needs connection lifecycle, back-pressure and size limits. Not needed once E + G are done |
| I. Log less | log `status` at most once per N seconds or at debug level | daemon.log growth 8 MB/day per dashboard to about 0; minor CPU | small |

What a Rust dashboard (ratatui) would fix: the dash side only. A ratatui program with no React tree, no development mode, no per-frame JS layout and a binary tty write per changed cell would be expected at about 1 to 2% CPU while working and about 10 to 20 MB RSS (est., from typical ratatui programs; **not measured, no Rust toolchain on this machine**). That removes causes 1, 2, 4 and 6 completely and cause 5's parse cost.

What it would **not** fix: causes 3 and 7, i.e. the daemon. Two Rust dashboards at 2 s would still cost the daemon about 16 points of CPU (2 x 8) and, on a larger live ledger, more. Rust also needs its own JSON model of the status contract (about 950 lines of `model.ts` and 1,370 lines of `view.ts` to port), a second build/release path and a second test set.

Recommendation for the decision (my judgement from the data): do **A**, **E** and **C/B** in Node first. A alone halves the dash and removes the leak; E cuts the daemon cost per dashboard by about 85%; C/B should bring the working case near 10 to 15%. If the dash is still above about 5% idle after that, a Rust dash becomes worth building, and it would then also need E (or G/H) to be cheap in total. A and E are independent, small, and can be shipped in separate commits.

## Incident during this job

While setting up the scratch project I ran `cstan ping` from the scratch directory with `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` still exported in my shell (they point at the live capstan project). The client prefers the inherited socket, so the ping reached the **live** daemon (`pong`, pid 27263), and I then sent SIGTERM to that pid believing it was a scratch daemon. The live controller stopped until the user restarted it. I did not touch the ledger. Two later `pkill -f` calls with patterns that also matched my own shell command line killed my own shell, not other processes (checked: the live daemons were still running afterwards); I stopped using `pkill -f` and signalled only pids whose `/proc/<pid>/cwd` I had checked.

Guards that would have prevented it:

* `cstan ping` (and `status`) should print the project root and ledger path it talked to, so a ping answered from `/home/akhil/Workspace/.../capstan` is obvious.
* An operator command run inside a directory that has its own `.capstan/` should refuse, or at least warn, when `CAPSTAN_SOCKET` points at a different project's socket.
* Before any `kill`, compare `/proc/<pid>/cwd` and the daemon's ledger path with the scratch project; never use a pid that was only reported by a ping.

## What I could not measure

* **The real ledger.** A copy of the live ledger was denied by the permission classifier for the first request, and when later allowed by the PM the daemon would not start on the copy: the copy's operator credential does not match the scratch project's, and changing it in the copy was denied. So all daemon numbers use a synthetic 450-agent ledger, not the live one (live ledger: 58 MB, stateVersion 19,869). Real per-request cost is probably higher, because the `roles` query is quadratic in seats x actors.
* **Plan-19 and plan-21 queries** (`plans`, `plan_packages`, `plan_revisions`, `plan_signoffs`, `activeTasks` with external links, `pipelineCounts` for integrations/reviews): the scratch ledger has no plans, findings, reviews or integrations. They showed under 1 ms per request on empty tables, so their cost at scale is **unknown**. The profile does not cover them.
* **Real herdr cost.** The scratch daemon ran with no `herdr` on `PATH`, so the driver tick has no per-agent pane observation, process sampling or delivery; the live daemon's idle CPU is higher (the live one showed about 11% average in `ps`).
* **Working agents on the real daemon.** The real daemon ended all agents without a pane, so the "agents working" cases used a replay server (`fake-daemon.mjs`) for the dash side; the daemon-side cost with working agents is the same request, so I did not repeat it.
* **Terminal emulator cost and real tty write cost.** The harness uses a pty read by Python; the 35% `writeUtf8String` share may be inflated or deflated on a real terminal. The emulator process's own CPU is not counted.
* **Why dev runs showed 17 KB/s of tty output early and 1.4 to 3.9 KB/s later.** Unexplained; not used in any conclusion.
* **A heap snapshot** of the dash: the leak cause (React dev `performance.measure` entries) is shown by a counter and by the production run, not by a heap diff.
* **The daemon RSS step from 196 MB to 298 MB** during the session: observed, not investigated.
* **Rust.** No toolchain; the Rust figures are estimates.
* **Which mode the user's dash actually runs in.** I did not inspect the live install's launcher; if the user runs the packaged binary, cause 1 does not apply to them and the numbers in the production tables are the relevant ones.
* **Single machine, single run per cell** (the 60 s cells are one run each); differences under about 2 percentage points are noise.

## Reproduce

All tools are in `docs/research/dash-bench/`. Run with `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` unset, in a scratch project, never in a live one.

* `dash-bench.py --project DIR --cli dist/src/cli.js --dashes N --seconds S [--interval I] [--extra=--reduced-motion] [--cpu-prof DIR] [--daemon-pid PID]`: CPU/RSS/tty bytes for N dashes and the daemon.
* `status-probe.mjs PROJECT_DIR [N]`: response bytes, latency, parse and model-build time.
* `fake-daemon.mjs SOCKET STATUS_JSON [WORKING]` and `dash-run.mjs`: dash against a replayed status with N agents working (`FAKE_SOCK=SOCKET node dash-run.mjs dash`).
* `cpuprofile-summary.mjs FILE`, `cpuprofile-inclusive.mjs FILE names`: summarise `--cpu-prof` output.
* `count-writes.mjs`, `perf-entries.mjs`: `node --import` preloads for tty write counts and React perf-entry growth.
* `query-timing.mjs COPY.sqlite PROJECT_ID`: times the `roles` and agent queries on a copy of a ledger (`PLAN=1` prints the query plans).
