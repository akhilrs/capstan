# Daemon background cost in v0.3.0: where the CPU and memory go when nobody is calling it, and what was cut

Package plan-24/daemon-bg-profile, 2026-10-08. The v0.3.0 daemon (plan-23 had already taken the `status` and SQL costs out) still uses a steady share of a core and a couple of hundred MB with no client calling it. This report attributes that to named loops, functions and child processes on a copy of the live ledger, with the Herdr calls answered by a replay shim, fixes the top items without changing any result, and measures baseline and after on the same copy with the same commands.

Everything was measured on this shared 8-vCPU machine (other agents and another project build and test on it) and on a **copy**. Nothing was sent to the live daemon or to the live Herdr and no live file was written; the complete list of accesses is in "Method and safety". The numbers are not CI-reproducible; the ratios are stable, and every repeat is listed.

## Result

There are two scenarios, both idle (no client call, active agents answered by the replay shim, 10 minutes per run). **Scenario A** is a ledger copy of 08:30 with six active agents and one unread PM message (the heavy case; first round of runs, code at commit 0b6e56f). **Scenario B** is a second copy of 12:52, after the PM's follow-up, when the live ledger had moved on to three active agents and no unread PM message (lighter; the final code, commit a71109c). Scenario A: baseline = v0.3.0 (commit 8b04ed0, started with no node flags), after = commit 0b6e56f (started the way its own daemon spawn starts it, with `--max-semi-space-size=2`). "CPU" is the daemon process plus the child processes it reaped (`utime+stime+cutime+cstime` of `/proc/<pid>/stat`), as a percentage of one core, the mean of the ten one-minute windows of the run. RssAnon is read from `/proc/<pid>/status` at the end of the run, 10 minutes after the start (MB here is MiB, `kB / 1024`).

| Run | Window (local, 2026-10-08) | Load 1/5/15 at start, at end | CPU | RssAnon | Counts for the targets? |
| --- | --- | --- | --- | --- | --- |
| final-base-1 | 09:55:01-10:05:21 | 4.79 6.50 8.23, 2.87 3.60 5.89 | 9.03% | 227.4 MB | yes (primary pair, no spike) |
| final-after-1 | 10:05:21-10:15:41 | 2.87 3.60 5.89, 6.12 5.51 5.85 | 2.21% | 45.8 MB | yes (primary pair) |
| final-base-2 | 10:15:41-10:37:17 | 6.12 5.51 5.85, 44.21 58.00 63.27 | 9.48% (invalid) | 96.2 MB (invalid) | **no**: an external load spike (1-minute load average up to 192) stalled the daemon for minutes (windows of 62, 74 and 65 s, 2 herdr calls in a minute); a replacement run is final-base-4 below |
| final-after-2 | 10:37:17-10:47:36 | 44.21 58.00 63.27, 2.04 9.49 33.75 | 2.16% | 46.7 MB | yes, though it started inside the spike's tail: the idle daemon's CPU did not move (2.0 to 2.5% in every minute) |
| final-base-3 | 10:47:36-10:57:56 | 2.04 9.49 33.75, 1.95 2.82 18.34 | 8.97% | 229.3 MB | yes (load 1.7 to 2.1 in the run) |
| final-after-3 | 10:57:56-11:08:16 | 1.95 2.82 18.34, 5.52 4.35 11.36 | 2.19% | 46.4 MB | yes |
| final-base-4 (replacement for base-2, after the window) | 11:08:53-11:19:05 | 4.48 4.23 11.02, 15.68 14.24 13.10 | 8.37% | 275.1 MB | non-quiet (the window had ended), shown for completeness |

Every window above is inside the PM-granted quiet window: message 3346f04d-e095-4af3-9c3b-03fada6bc3f2 granted it from 04:20 UTC (09:50 local), message d1254a44-33a7-4953-b189-71615b555339 corrected the start to 04:24 UTC (09:54 local) and the end to about 05:40 UTC (11:10 local), and message 4077f34f-d00f-4cba-805e-519a80fabdcb said to report per-run numbers with their load averages, label them non-quiet as the plan says and say explicitly whether the targets hold under that load. A run I began at 09:50:20, before the corrected start, was stopped at 09:53 and is kept only as `NONQUIET-aborted-base-1` (not in this report). **The machine's load average stayed above the PM's "2" line in most runs (other users' agents are outside the hold), so by that rule these runs are non-quiet and are labelled so here; the targets hold under that load**, with the interleaved before/after pairs seeing the same load and the after daemon's CPU not moving at all between load 2 and load 60 (2.16, 2.19, 2.21%).

| Target (acceptance 5) | Baseline | After | Verdict |
| --- | --- | --- | --- |
| idle background CPU at least 50% lower | 9.03% and 8.97% (mean 9.00%) | 2.21%, 2.16%, 2.19% (mean 2.19%) | **met: 75.7% lower** |
| RssAnon after 10 minutes at least 30% lower | 227.4 MB and 229.3 MB (mean 228.4 MB) | 45.8, 46.7, 46.4 MB (mean 46.3 MB) | **met: 79.7% lower** |
| contained fixes (at most 16 of the 40 hours) | n/a | four small changes in `src/driver.ts`, `src/reports.ts`, `src/herdr/process-activity.ts`, `src/client.ts` (plus the new `src/node-options.ts` and one line each in `src/cli.ts` and `src/sea.ts`, architect-22 approved), about 300 changed lines of source | met |
| results equal the pre-fix code on a deterministic fixture, spawn counts budgeted | n/a | `test/daemon-background.test.ts`, below | met |

### Scenario B (final code), in the quiet window granted by message 43e61308-44ed-4f78-90a5-3318edab8192

The grants: message 43e61308-44ed-4f78-90a5-3318edab8192, "quiet window START now, 08:07 UTC (13:37 local), until about 09:25 UTC (14:55 local), all other builds held", and, after an external spike ended the first one, message 18c5e5c2-6cb0-44f4-9b4e-2f6f60dd463f, "window GO now, 09:20 UTC (14:50 local), until about 10:05 UTC (15:35 local)". Three agents are active in this copy (`architect-7` and `pm-1` idle, `supervisor-1` working, so the probe samples one agent every 15 s and there are 1.5 `agent get` spawns a second); the PM has no unread message, so no wake is outstanding and the 128 MB young generation of scenario A does not grow in the baseline (8 MB committed); what is left of the baseline is the process-table scan and the spawns.

| Run | Window (local) | Load 1/5/15 at start, at end | CPU | RssAnon | Counts for the targets? |
| --- | --- | --- | --- | --- | --- |
| final-base-1 | 13:37:42-13:48:02 | 1.20 1.61 4.99, 2.24 2.90 4.19 | 3.24% (ten windows 3.0-3.5) | 102.4 MB | yes |
| final-after-1 | 13:48:02-13:58:22 | 2.24 2.90 4.19, 1.56 2.02 3.14 | 1.28% (ten windows 1.1-1.5) | 40.3 MB | yes |
| final-base-2 (a, spike) | 13:58:22-14:12:07 | 1.56 2.02 3.14, 58.74 75.04 38.23 | minutes 1-6: 3.2-3.7%; then an external spike (load 28, then 158) stalled the daemon: 4 herdr calls a minute at 10.7% system CPU | not read | **no** |
| final-after-2 (a, spike) | 14:12:07-14:24 (stopped by me) | 54.84 73.96 38.07 at the start, 80+ later | minutes 1-4: 1.3-1.6%, then stalled the same way | not read | **no** |
| final-base-2 (b) | 14:50:28-15:00:47 | 1.47 13.20 33.73, 3.01 3.91 18.39 | 3.25% (ten windows 3.0-3.5, 119-124 herdr calls each) | 107.7 MB | yes (load 1.9-6.9 inside the run) |
| final-after-2 (b) | 15:00:47-15:17:19 | 3.01 3.91 18.39, 206.77 158.32 81.08 | minutes 1-8: 1.2-1.6%, mean 1.36% (60 s windows, 116-124 herdr calls each, load 2-17); minutes 9-10 stalled by a new external spike (window of 429 s) | 40.1 MB (read at 16.5 minutes, after the stall; the memory had stopped changing) | CPU: the eight clean minutes only; RssAnon: yes |
| final-base-3 | not run | n/a | n/a | n/a | I stopped the sequence at 15:17 as instructed, with the load at 206 |

Scenario B means over the runs that count: CPU baseline 3.24 and 3.25% (mean **3.25%**), after 1.28 and 1.36% (mean **1.32%**): **59.4% lower**; RssAnon baseline 102.4 and 107.7 MB (mean **105.1 MB**), after 40.3 and 40.1 MB (mean **40.2 MB**): **61.8% lower**. Both targets hold in this lighter scenario too, with two repeats instead of three (two completed pairs fell to the external spikes). The baseline is lower than scenario A's because there is no outstanding wake and fewer agents; the fixes still remove the process-table scan (60.6 MB of `[heap]` and 16 MB of `external` in the baseline against 5.8 and 2.2 MB after) and the part of the spawn cost that depended on the resident size.

Under the plan-23 mix (the operator's `status` at 0.36 per second, the rate the live `daemon.log` shows) the numbers are higher for both builds and I did not repeat them in the window: the status path is plan-23's and is not changed here.

## What the 9% is, and what the fixes do

The idle daemon runs three loops at 2 s (the delivery driver, the report relay, and, at 15 s, supervision) and does one thing in them that costs real time on a machine with this many processes and this many messages:

1. every tick the driver starts one `herdr agent get` per active agent (six on this ledger, three per second): a `spawn` from a process whose resident size, and so fork cost, was 250 MB;
2. every 15 s per working agent the process probe starts a `herdr pane process-info` and then **reads `/proc/<pid>/stat` of every process on the machine** (480 files here, around 2000 on the busiest moments of the day) to find the few processes under one pane;
3. while a PM message is waiting for its wake (the ledger copy has one) the driver reads **all 1046 of the PM's messages** twice a tick (`#judgeWakes`, `#judgeStale`);
4. the report relay runs five to ten statements every tick even though nothing it looks for can change without a ledger mutation;
5. the V8 young generation grew to two 64 MB semi-spaces from the bursts of those message rows and process-table strings, stays there, and is resident, which also makes every fork dearer.

The fixes, all in the owned files and none changing a result:

* the driver caches a PM's mail by ledger version (`core.stateVersion`, which every mutation moves) and keeps only a digest (state per message, latest pull time) of the whole mail instead of the rows; the open-mail reads of `#deliver`, `#isHead` and `#judgeStale` use the same cache;
* the report relay does not repeat a pass over a ledger version at which its last full pass found nothing to announce (finding sweeps, which depend on the clock, still run each tick);
* the process probe follows `/proc/<pid>/task/*/children` from the pane's shell pid instead of reading every process; where the kernel has no `children` lists it falls back to the whole-table scan;
* the daemon is spawned with `--max-semi-space-size=2` (via `NODE_OPTIONS` for the standalone binary).

## Method and safety

Live access, in full (the raw command log is `docs/research/daemon-profile-v030/results/live-access.log`; it shows only what is listed here):

* **`/proc` reads** of the live daemon (pid 2433, working directory `/home/akhil/Workspace/github.com/akhilrs/nexora`): `/proc/2433/{cwd,stat,status}`, `/proc/2433/task/*/stat`, `ls /proc/2433/task`, `ls -l /proc/2433/fd | wc -l`, `/proc/loadavg`, a process-list `ps` (also `/proc`). One five-minute pair of samples (09:19:32 and +300 s, `live-sample.sh`) gives the live `utime/stime/cutime/cstime` cross-check.
* **`daemon.log` reads**: `tail -n 3000` of the live `.capstan/daemon.log` (twice), analysed for call rates.
* **The `copy-ledger.mjs` run**: `node scripts/copy-ledger.mjs <live controller.sqlite> /tmp/capstan-prof-v030/ledger` (SQLite backup API over a read-only connection, 0700 directory, `integrity_check` ok, 73.9 MB). The live `operator.key` was not copied, read or touched: the copy's operator `credential_hash` was rewritten to the hash of a fresh scratch credential, as in plan-23 (the PM agreed then).
* Directory metadata (`ls -la` of the live `.capstan` and `.capstan/state`), the live `capstan.toml` (read; copied without `[operator]` into the scratch project), an existence check of one worktree path named in the ledger, and the mtime of the main checkout's `dist/src/cli.js`.
* **No herdr command ran against the live session**, and none was asked of the PM. Not accessed: `operator.key`, the live ledger itself except through `copy-ledger.mjs`, `control.sock`, any live pane or worktree.

The scratch project (`docs/research/daemon-profile-v030/setup.sh`) lives in a 0700 tree under `/tmp/capstan-prof-v030/`: the copied ledger, the scratch credential, a `git` shim that refuses any path outside `/tmp` (it refused two operations on a live worktree path named in the ledger, see the log), and a **Herdr replay shim** (`herdr-replay`) first on `PATH`. The daemon runs under `env -i` with only `PATH`, `HOME`, `LANG`, `TMPDIR`.

**The Herdr replay shim** (acceptance 2). The ledger has six active agents (`architect-7`, `developer-89`, `developer-90`, `developer-92`, `pm-1`, `supervisor-1`) with panes. The shim answers, from `replay.map` built out of the ledger copy:

* `agent get <slug>-<agent>` with `agent_status` (`working` for the three developers and the supervisor, `idle` for the PM and the architect) and the recorded `pane_id`, in the shape the adapter reads;
* `pane get`, `pane read` (the screen is `test/fixtures/claude-idle-empty.ansi`) and a generic `{result:{}}` for everything else the launcher's adoption pass sends (`workspace rename`, `report-metadata`, `agent prompt`, `notification show`);
* `pane process-info --pane P` with the shape of `test/fixtures/herdr-process-info.json`, whose `shell_pid` is the pid of a **real scratch process tree** per agent (a shell, a host, a tool shell running `sleep`), so the probe reads the real `/proc` of this machine exactly as it does live.

Every call is logged by arguments only (shell builtins, so the log itself spawns nothing). The Herdr-dependent driver paths that plan-23 could not reach (per-agent `agent get`, process-activity sampling, adoption at start) run on the copy: about 207 to 222 shim calls a minute (3.4 to 3.7 a second: 2.97 `agent get` plus 0.27 `pane process-info`).

Commands (all in `docs/research/daemon-profile-v030/`, raw output under `results/`; `reset.sh`, `start.sh`, `memreport.mjs`, `top.mjs`, `heap.mjs` are plan-23's, unchanged, run from `../daemon-profile/`):

```sh
node scripts/copy-ledger.mjs <live controller.sqlite> /tmp/capstan-prof-v030/ledger
setup.sh <ledger copy dir> <live project root> <checkout> /tmp/capstan-prof-v030/s   # scratch project, shims, trees
run.sh <label> <dist> <minutes> plain|profile [node flags]   # one run: per-minute CPU (own + children, per thread), mem, memreport, shim call counts
final.sh <baseline dist> <after dist> 3 10                   # the interleaved final sequence
attribute.mjs / account.mjs / summarize.mjs                  # profile owners, the CPU split, the table lines
connections.mjs <pid> <n>                                    # RssAnon per idle connection and per status call
memsplit.mjs <pid>                                           # RssAnon by kind from smaps
live-sample.sh <live pid> <seconds>                          # the only live reads: /proc of the live daemon
```

Machine and versions: Linux 6.1.0-48-amd64, QEMU Virtual CPU 2.5+, 8 vCPUs, 16 GB, Node.js v24.6.0, SQLite 3.50.4 (inside `node:sqlite`), `CLK_TCK` 100, 480 processes and about 1900 tasks at run time. `--cpu-prof` adds a profiler thread of its own (`v8:ProfEvntProc`, 1.4% of a core), so profiled runs are used only for the **shares** inside the main thread; every figure of CPU in the tables comes from plain runs without a profiler.

## Baseline attribution (CPU)

`account.mjs` splits the `/proc` CPU of a plain baseline run by thread (main thread, the four V8 worker threads, the four libuv worker threads, children) and splits the main thread by the owner shares of the profiled baseline run (`base-idle-prof`, 10 minutes, `--cpu-prof`; owners are the nearest named frame on the stack, async `fs` callbacks go to the process probe, the only user of `fs.promises` in the loops). Baseline, final-base-3 (final-base-1 is within 0.1 point of every line):

| Cost | % of one core | Share of the 8.97% |
| --- | --- | --- |
| process-activity probe, main thread (`/proc` scan: ~2000 `readFile`/`fstat`/`close`, `parseProcStat`, plus the `pane process-info` spawn) | 2.12 | 24% |
| `herdr agent get` per agent per tick, main thread (`spawn`, pipes, strict UTF-8 decode, JSON) | 1.68 | 19% |
| child processes of the daemon (the shim; `cutime+cstime`): 90% `agent get`, 8% `process-info` | 1.30 | 14% |
| driver PM-mail re-read (`#judgeWakes`, `#judgeStale`: `messagesFor`, 1046 rows with bodies, twice a tick) | 1.01 | 11% |
| libuv worker threads (the async `fs` reads of the `/proc` scan) | 0.77 | 9% |
| V8 GC worker threads (parallel scavenges and marking) | 0.61 | 7% |
| report relay tick | 0.60 | 7% |
| V8 GC and native code on the main thread | 0.39 | 4% |
| driver `#advance` (`advanceMessaging`, `queueMissingDeliveryNotices`) | 0.20 | 2% |
| other main-thread work (`#deliver`, supervision, request handling, startup, sqlite helpers) | 0.22 | 2% |
| **attributed to a named loop, function or child process** | **8.67** | **97%** (99% counting the last line) |

Acceptance 3 asks for at least 80%: 97% is attributed to named items. The per-function evidence is in `results/base-idle-prof-attribution.txt` (owner shares and the plan-23 `top.mjs` self-time list). The live cutime/cstime cross-check is in the next section.

After (final-after-3, 2.19%): the same table in `results/final-after-3-account.txt`: `agent get` spawns 0.70 (32%), child processes 0.69 (32%), `#advance` 0.20 (9%), other main-thread work 0.20 (9%), GC on the main thread 0.10 (5%), the process probe 0.09 (4%), report relay 0.04 (2%), PM-mail re-read 0.04 (2%), V8 and libuv worker threads about 0.01 together. Two thirds of what is left is the six `herdr agent get` spawns per tick.

Per-tick spawns (acceptance 3, "per-tick herdr spawns"): 2.97 `agent get` (0.5 a second per agent) and 0.27 `pane process-info` (one per working agent per 15 s) a second, unchanged by the fixes (the cadence is kept). Their cost fell with the process size: the JS side of the spawns went from 1.68 to 0.70 points and the children from 1.30 to 0.69.

## RssAnon split

Sources: `memreport.json` of every run (V8 spaces and `/proc/<pid>/smaps` by mapping, written on `SIGURG` by plan-23's `memreport.mjs`), `memsplit.json` of final-base-4 (smaps by kind) and `connections.txt` (RssAnon per connection). The V8 heap itself is small in both builds (live heap 14.9 MB in the baseline snapshot, `results/base-idle-prof-heap.txt`: code 4.4, strings 4.2, native 1.9 MB); the baseline's RssAnon is mostly **garbage the young generation never gave back** and **memory the allocator kept after the process table scans**.

| Part of RssAnon (10 minutes in, idle) | Baseline | After | What it is |
| --- | --- | --- | --- |
| V8 young generation (new space) | about 123 MB resident, 128 MB committed (2 x 64 MB semi-spaces), 3 to 10 MB used when sampled | about 4 MB committed, 1.9 MB used | V8 grows the semi-spaces when a tick holds a few MB of rows (1046 messages with bodies, process-table strings) while it allocates more, and does not shrink them; the cap holds them at 2 MB each |
| V8 old generation, code, large objects | about 15 MB (old 7.8 to 10.3, code 3.8, large objects 0.7) | about 17.7 MB (old 14.3 committed, 10.5 used; code 3.3; large 0.1) | what the daemon really keeps; slightly more after because a 2 MB young generation promotes more, and the version-keyed caches live here |
| outside the heap: glibc main arena (`[heap]`) | 99.4, 115.4 and 124.6 MB (final-base-1, -3, -4) | 7.3 to 7.4 MB | SQLite page caches, file-read buffers and strings from about 2000 `/proc/<pid>/stat` reads per sample that the allocator keeps; it grows with the number of scans |
| outside the heap: glibc thread arenas | 11.5 MB in 12 arenas (final-base-4) | not measured (the run that would have measured it was paused) | the libuv and V8 worker threads' arenas |
| rest (stacks, V8 metadata, `external`) | the remainder: 227.4 minus the lines above | 45.8 - 4 - 17.7 - 7.4 = about 16.7 MB including the thread arenas | |
| **RssAnon** | **227.4, 229.3 and 275.1 MB** | **45.8, 46.7 and 46.4 MB** | |
| RssFile (the node binary and libraries; not RssAnon) | 27 to 54 MB | 52 to 54 MB | a Rust daemon would map about 5 MB |

Scenario B (three agents, no outstanding wake, final-base-1 and -2): the young generation stays at 8 MB committed because no 1046-row read happens, so RssAnon is 102.4 and 107.7 MB and is mostly outside the heap: `[heap]` 60.6 MB, `external` 16.1 MB, old generation 11 MB; after the fixes 40.3 MB with `[heap]` 5.8 MB, `external` 2.2 MB, old generation 12.8 MB, young generation 1 MB.

Per connection (`connections.mjs`, 60 concurrent connections, the daemon's limit is 64): an idle connection costs **2.5 KB** in the baseline and 0.1 KB after (the socket and its parser state). A burst of 60 concurrent `status` calls raised RssAnon by 39.4 MB in the baseline, **672 KB per connection**, and it was still resident five seconds later (155.0 MB against 115.2 MB before), because every `status` response allocates rows and a JSON string in the young generation; with the cap the same burst raised it by 4.0 MB, **67.5 KB per connection**.

The fork cost the spawns pay is proportional to the resident size: a microbenchmark of `spawn('/bin/true')` from a node process of 57, 108, 157 and 258 MB RSS used 0.60, 0.99, 1.53 and 2.58 ms of system time per spawn in the parent (plus 0.8 to 0.9 ms of user time), which is why the cap also made the `agent get` spawns cheaper.

## Cross-check against the live process (acceptance 3, "live cutime/cstime as a cross-check")

The live daemon (pid 2433) was started on 2026-10-04 23:09, **before** plan-23's fixes and the v0.3.0 release, so it runs v0.2.0-era code. The like-for-like comparison is therefore the v0.2.0 build (`git archive 6881413`, compiled to `/tmp/capstan-prof-v030/dist-v020`) on the same copy and shim with `status` replayed at the live rate, against the live `/proc` over the same kind of window. A five-minute pair of reads of the live process (09:19:32 local, 300 s) and a five-minute v0.2.0 run (non-quiet, load 6 to 19):

| | live daemon (pid 2433, v0.2.0 code) | scratch v0.2.0 + shim + `status` 0.36/s | scratch v0.3.0 idle (baseline) |
| --- | --- | --- | --- |
| user / system CPU | 25.8% / 13.2% | 15.7% / 10.6% | 4.8% / 3.1% |
| `cutime + cstime` (children) | 0.13% + 0.74% = **0.87%** | 1.0% | 1.25% |
| total of one core | **39.9%** | **26.8%** | 9.0% |
| RssAnon / RssFile | 265 MB / 30 MB | 247 MB / 53 MB | 227-229 MB / 27-54 MB |
| `status` calls (live `daemon.log`, last 3000 lines) | 0.361/s, mean 263 ms | 0.36/s, p50 165 ms | none |

The shim reproduces 67% of the live CPU and 93% of its RssAnon with the same code, and the **children's CPU matches** (0.87% live against 1.0% scratch): the real `herdr` process costs the daemon about as much as a `dash` script on the daemon's side of the fork, so the child-process line above is not an artefact of the shim. The remaining 13 points of the live CPU are not reproduced (see the last section): they are live-only. The live daemon's own v0.3.0 behaviour is not observable without restarting it, which I did not do.

## Per cost item: does Rust remove it, can Node fix it, what should phase 2 port first

Baseline cost in points of one core (final-base-3) and after (final-after-3):

| Item | Baseline | After | Does Rust remove it | Can Node fix it (done?) | Phase 2 |
| --- | --- | --- | --- | --- | --- |
| `/proc` scan of the process probe (every process's `stat` per sample per working agent) | 2.89 (main 2.12 + libuv 0.77) | 0.09 | The scan is an algorithm, not a runtime cost: Rust would run it faster (no `fs.promises` round trips) but would still read 480 to 2000 files; the fix (follow the children lists) is the same in either | Yes, done: a handful of reads (the test caps it at 20) instead of one per process; falls back to the table where `children` is missing | not first: the fix ports in an afternoon |
| `herdr agent get` per agent per tick, JS side + children | 2.98 (1.68 + 1.30) | 1.39 | Mostly: a Rust daemon spawns with `posix_spawn` at a fixed ~0.3 ms whatever its size, and if Herdr's CLI speaks to a local socket the daemon can use the socket directly and spawn nothing at all (the CLI's JSON ids, `cli:agent:get`, suggest one; not verified, it needs a live Herdr) | Partly, done: the cap cut the process from 250 MB to 47 MB, which halved fork cost; the exec of an external CLI six times per tick cannot go away in Node without changing the poll cadence, which is out of scope | **first**: the Herdr client (one `agent list` or a socket subscription instead of six spawns) is two thirds of what is left |
| PM-mail re-read (1046 rows with bodies, twice a tick while a wake is outstanding) | 1.01 | 0.04 | A ledger crate with prepared statements reads the rows in a fraction of the time, but still would re-read them without a version check | Yes, done (version-keyed digest) | the ledger crate; carry the version check |
| report relay tick (40+ statements before plan-23, 5 to 10 now, every 2 s) | 0.60 | 0.04 | No by itself; Rust makes each statement cheaper | Yes, done (skip a pass at an unchanged ledger version) | with the ledger crate |
| V8 GC: the young generation grew to 2 x 64 MB, resident; GC worker threads | 0.61 workers + 0.39 main = 1.00 | 0.11 | **Yes, entirely**: there is no GC | Yes, done: `--max-semi-space-size=2` at daemon spawn | n/a |
| `advanceMessaging` + `queueMissingDeliveryNotices` (a scan of all messages whose state is not final, joined to agents, 3.6k rows) | 0.20 | 0.20 | Only through the ledger crate's query plan | Not in the owned files: it needs an index on `messages(state)` or a rewritten query in `src/controller`, and the index needs a migration, which the plan forbids without the PM | second: the ledger crate, with the index |
| other main-thread work | 0.22 | 0.20 | partly | no | with the daemon port |
| fixed cost of the runtime: RssFile 52 MB (the node binary), V8 code and old space ~14 MB | n/a (memory) | n/a | **Yes**: ~5 MB for a Rust daemon | no | the whole daemon port |
| memory per connection | 2.5 KB idle; 672 KB per concurrent `status` | 0.1 KB; 68 KB | Yes: no per-call young-generation churn | Done by the cap (10x) | n/a |

## The fixes

1. **`src/driver.ts`: PM mail by ledger version.** `#openMessages(agentId)` returns `openMessagesFor` as read at the current `core.stateVersion`; `#pmDigest(pm)` returns, for the PM's whole mail, a map of message id to state and the latest `sentAt`/`ackedAt` time, also keyed by version. `#judgeWakes` compares the digest with each outstanding wake exactly as it compared rows (`state !== "queued"`, `latest pull > wakeAt`, the same `NaN`-ignoring comparison), so the same wakes are answered at the same ticks. `#deliver`, `#isHead` and `#judgeStale` use the open-mail cache. The caches drop agents that are no longer active. Every `messages` write (three statements, all inside `mutate`) moves `state_version`, so a list read at the current version is current.
2. **`src/reports.ts`: a quiet pass is not repeated.** `quietAtVersion` records the ledger version at which a full pass found no report, review, plan notice or finding notice to announce (or no single active PM); a tick at that version returns after the finding sweep, which depends on the clock and still runs.
3. **`src/herdr/process-activity.ts`: `ToolProcessWalker` (`readToolProcesses(shellPid)`).** Follows `/proc/<pid>/task/*/children` from the pane's shell to the same processes `toolProcesses(readProcTable(), shellPid)` counts (a shell at depth two, plus all its descendants; MCP-server siblings excluded). `HerdrProcessProbe` uses it when constructed with the default table reader (production). The walker asks the kernel once, for good, whether it has `children` lists (it reads its own process's list: `ENOENT` means no `CONFIG_PROC_CHILDREN`) and then reads the whole table for ever; any other failure (a transient `EMFILE`, a permission error) makes that sample read the whole table and the walker tries the lists again after 60 s, so a one-off error no longer latches the fallback (my first version latched on any error, and treated a missing `children` file like an exited thread). A probe given its own table reader (every existing test) is unchanged.
4. **`src/client.ts` and `src/node-options.ts`: the young-generation cap at daemon spawn, and its removal from the daemon's environment.** `daemonCommand()` builds the spawn: node runs get `--max-semi-space-size=2` in front of `cli.js` (a node argument, so no child inherits it); the standalone binary has no node flags of its own, so the flag is appended to `NODE_OPTIONS` (documented as honoured by single-executable apps, `execArgvExtension` default `env`) and named in the variable `CAPSTAN_ADDED_NODE_OPTION`. The daemon calls `restoreNodeOptions()` as the first thing in its command in `src/cli.ts` (one line, architect-22 approved; it comes before `loadOperator` and before the Herdr runner copies the environment): V8 has read the flag by then, and the call removes exactly that flag and the marker from `process.env`, so the Herdr, git, launcher and restart-helper children do not inherit it. A user's own `NODE_OPTIONS` stays byte-identical, an emptied one is deleted, a user's own `--max-semi-space-size` is respected (nothing is added, nothing removed), and a node run (no marker) is a no-op. One consequence: a standalone daemon that restarts itself (the restart helper re-executes from the daemon's restored environment) comes back **without** the cap until the next `ensureDaemon` start; the node-run daemon keeps it through `process.execArgv`.

The restart machinery, the rest of `daemon.ts` and `cli.ts`, and the controller are not touched.

## Tests

`test/daemon-background.test.ts` (new, with `test/fixtures/daemon-background/drive.mjs` and `golden.json`), part of `npm test`:

* **The driver does what it did before the fixes.** `drive.mjs` runs a real `DeliveryDriver` for 80 ticks on the generated large ledger of `test/fixtures/daemon-cost` (500 agents, a PM with 1046 messages, one unread PM message) with a scripted Herdr and a virtual clock; two PM mails, one mail to a busy and one to an idle developer are enqueued, the PM pulls its first message at tick 30. It records every Herdr call, process-probe call, the driver's log events, notifications, snapshot, the ledger rows it changed and the event counts. `golden.json` was made with commit 8b04ed0 (the code before the fixes); the test asserts the current code returns the identical result (7 `agent get` per tick at most, 560 in all; 30 `pane process-info`; 2 wakes; 79 sends), and that a quiet tick reads at most 400 rows (before: 1355; after: 268) and every tick at most 260 statements (before and after: 106 to 142, 212 and 220 on the first tick). Run against the pre-fix build (`CAPSTAN_DAEMON_BACKGROUND_DIST`) the parity check passes and the row budget fails.
* **The process probe finds the same processes as the whole-table scan.** A real tree (shell, host, tool shell running `sleep`, a non-shell sibling) is started; `readToolProcesses` and `toolProcesses(readProcTable())` return the same pid, parent, name and start key, the sibling is not counted, and the targeted reader reads at most 20 files where the scan reads 10 times as many.
* **The daemon starts with the cap exactly once**, and the follow-up tests (PM review): for the standalone binary's `NODE_OPTIONS` path, `daemonCommand` sets the flag and the marker, `restoreNodeOptions` takes exactly those out and leaves the user's own `NODE_OPTIONS` byte-identical (an emptied one is deleted, a user's own `--max-semi-space-size` is respected and nothing is added, a node run sets no marker and the restore is a no-op, a stale marker in the parent's environment never reaches the daemon, a changed `NODE_OPTIONS` is not mangled); **a heap-statistics stand-in for the SEA run** (no standalone binary is built here, so plain node reads the flag from `NODE_OPTIONS` exactly as a single-executable app does, `execArgvExtension` default `env`): the uncapped control grows its young generation past 16 MB in 2.5 s of churn, the daemon started with `daemonCommand`'s environment stays at or below 8 MB, restores its environment, and a child it spawns sees neither the flag nor the marker; a wiring test that the daemon command in `src/cli.ts` calls `restoreNodeOptions()` before `loadOperator` and before the Herdr runner copies the environment; and `ToolProcessWalker` tests with a scripted `/proc` (a kernel without `children` lists is asked once and never again, a transient `EMFILE` makes that sample read the table and the walker retries after the 60 s backoff).

`test/daemon-cost.test.ts` and its golden are unchanged and pass, as do the driver, daemon and observe suites (70 tests). `npm run check` (lint, format check, dash check, build and the whole suite, in the slot the PM granted, at nice 19 with `--test-concurrency=2`): **passed**, exit 0: 1688 tests, 1684 pass, 0 fail, 4 skipped (the existing skips), no leaked temp directories; eslint 0 errors (only the existing `max-lines-soft` warnings).

## What could not be reproduced, and differences from the live daemon

* **The real Herdr.** No herdr command ran live (the PM decision), so the cost of the real `herdr` binary's work, and of a loaded Herdr server making each `agent get` slow, is not measured. The children's CPU matches the live one (0.87% against 1.0%), the latency does not matter for CPU.
* **The live daemon is older code.** It is v0.2.0-era, so the live CPU (39.9%) is compared with the v0.2.0 build on the copy (26.8%, 67% of live). The 13 points not reproduced are live-only: the live ledger has grown since the copy (live `status` takes 263 ms against 165 ms here), the live `capstan.toml` has an `[operator]` section that I removed (its result poll and restart service are not running in the scratch daemon), real Herdr panes behave differently from a scripted `idle`/`working`, the real agents' process trees under each pane are deeper than my scratch trees (a shell, a host, one `sleep`), and other clients (PM, Supervisor, developers: 0.05 calls per second together) call the daemon.
* **The scratch trees are small.** The fix to the probe makes its cost proportional to the pane's tree instead of to the machine; the real trees are larger than mine, so the probe's cost after the fix is understated here (still far below the whole-table scan, whose cost does not depend on them).
* **The standalone binary** was not built, so `NODE_OPTIONS` honoured by the single-executable app is documented behaviour, not measured on a real binary; the stand-in test above uses plain node reading the same variable. A standalone daemon that restarts itself comes back without the cap (see the fixes).
* **Machine noise.** Load averages of 2 to 60 from other users; interleaving base and after and repeating three times kept the CPU readings within 0.05 points for the after daemon and 0.06 for the baseline, except final-base-2, which the load spike stalled.
* **Not reached in the owned files:** the `advanceMessaging` scan (needs an index), and the six `agent get` spawns per tick (the cadence is fixed by the plan).

## Measurement windows

| Window | Granted by | Start / end (local) | Load 1/5/15 at start, at end |
| --- | --- | --- | --- |
| Quiet window for the final runs | 3346f04d-e095-4af3-9c3b-03fada6bc3f2 (start 04:20 UTC), corrected by d1254a44-33a7-4953-b189-71615b555339 (04:24 to about 05:40 UTC); numbers-with-load rule in 4077f34f-d00f-4cba-805e-519a80fabdcb | final.sh restart 09:55:01, last run ended 11:08:16 | 4.79 6.50 8.23, 5.52 4.35 11.36 |
| Scenario B, window 1 | 43e61308-44ed-4f78-90a5-3318edab8192 (08:07 UTC, until about 09:25 UTC) | 13:37:42 to 14:24 (pair 1 clean; the rest hit an external spike) | 1.20 1.61 4.99 at the start |
| Scenario B, window 2 | 18c5e5c2-6cb0-44f4-9b4e-2f6f60dd463f (09:20 UTC, until about 10:05 UTC) | 14:50:28 to 15:17 (base-2 clean, after-2 clean for 8 minutes, then a second external spike; stopped by me) | 1.47 13.20 33.73 at the start |
| Aborted before the corrected start | n/a | 09:50:20 to 09:53 | 4.46 8.22 9.24 at the start |
| Exploratory and cross-check runs (not final, non-quiet, not counted) | none needed | exp-semi2, exp-new-plain, base-idle-prof, new-idle-prof, xcheck-v020-mix, connections | loads in `results/*-cpu.txt` |

## Files

`docs/research/daemon-profile-v030/`: `setup.sh`, `herdr-replay`, `run.sh`, `final.sh`, `bench.mjs`, `attribute.mjs`, `account.mjs`, `summarize.mjs`, `connections.mjs`, `memsplit.mjs`, `live-sample.sh`; `results/`: the final runs' `cpu.txt` (per-minute CPU with per-thread split and load), `mem.txt`, `memreport.json`, `proc-*`, shim call counts, the profile attributions, the heap summaries, `live-access.log`, the v0.2.0 cross-check, the connection probe, the golden-drive row and statement counts of both builds. Heap snapshots and cpuprofiles (under `/tmp/capstan-prof-v030`) were deleted after summarising; the scratch daemons and process trees were stopped and the tree removed before this report.
