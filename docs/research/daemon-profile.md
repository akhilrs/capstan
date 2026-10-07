# Daemon profile on a ledger copy: where the time and memory went, and what was fixed

Package plan-23/daemon-profile, 2026-10-07. The live daemon sat at about 43% CPU (12 points of it `status`, about 31 unattributed) and 320 MB RSS (258 MB anonymous). This report profiles the daemon on a copy of the live ledger, attributes both numbers to named functions, fixes the top costs without changing any result, and records baseline and after on the same copy with the same commands.

Everything measured here was measured on this machine on a copy. None of it is CI-reproducible: the machine is a shared 8-vCPU QEMU guest (other agents run builds and test suites on it; `uptime` read 12.6, 150 and 248 for the 1, 5 and 15 minute load averages while I worked), so absolute percentages move by several points between repeats (the repeats are listed). The ratios are stable.

## Result

| Target (acceptance 4)                                                               | Baseline                                   | After                                    | Verdict                                                                                                                                          |
| ----------------------------------------------------------------------------------- | ------------------------------------------ | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `status` p50 <= 30 ms and >= 4x faster (200 calls, idle daemon)                     | p50 134.5 ms, p95 473.1 ms                 | p50 23.8 ms, p95 28.2 ms                 | met: 5.7x                                                                                                                                        |
| daemon CPU under the replayed mix (status at 0.9/s), mean of 3 x 60 s, >= 60% lower | 27.3%                                      | 3.1%                                     | met: 89% lower                                                                                                                                   |
| daemon CPU idle (mean of 3 x 60 s), recorded                                        | 16.1%                                      | 0.7%                                     | 95% lower                                                                                                                                        |
| RssAnon after 10 minutes, >= 40% lower: idle                                        | 163.3 MB                                   | 37.9 MB                                  | met: 77% lower                                                                                                                                   |
| RssAnon after 10 minutes, >= 40% lower: under the mix                               | 165.8 MB (protocol), 101.5 MB (second run) | 51.8 MB (protocol), 69.4 MB (second run) | met in the protocol run (69% lower), **not** in the second run (32% lower): the baseline value is unstable (101 to 182 MB over five loaded runs) |

Of the 138 ms of a `status` call, 128 ms was SQL (several queries that read the whole of `messages`, `agent_reports`, `reviews` or `controller_events`); the two background loops (report relay every 2 s, delivery driver every 2 s) spent the same kind of time on the same tables. Result parity with the code before the fixes is a committed test (`test/daemon-cost.test.ts`, described at the end).

## Method and safety

Nothing was sent to the live daemon and no live file was written.

- `scripts/copy-ledger.mjs <source controller.sqlite> <target dir under /tmp>` took the snapshot. It opens the source read-only (as immutable when no `-wal` file exists beside it, so it never creates a `-shm` or `-wal` next to the source), runs the SQLite backup API into a fresh 0700 directory, runs `integrity_check` on the copy and folds its WAL. The live ledger was 67.8 MB with a 4.2 MB WAL at copy time (505 agents, 3351 messages, 23336 events, 287 reviews, 202 reports).
- The scratch project (`docs/research/daemon-profile/setup.sh`) lives in a 0700 directory under `/tmp`: the copied ledger, the live `capstan.toml` with `[operator]` removed, the roles directory, a git repository with one empty commit as workspace, and two shims first on `PATH`: a `herdr` shim that answers nothing (exit 0, empty output) and logs every call, and a `git` shim that refuses any absolute path or working directory outside `/tmp` and logs what it ran.
- The daemon runs under `env -i` with only `PATH`, `HOME`, `LANG`, `TMPDIR`; `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` are not set, and the daemon's own socket is inside the scratch directory. The live `operator.key` was **not** copied: the PM agreed to rewriting the operator actor's `credential_hash` in the copy to the hash of a fresh scratch credential, so the daemon opens the copy with a key that has never existed in the live project.
- Because the Herdr shim answers nothing, the driver finds no panes, so costs that depend on what Herdr says (per-agent `pane get`, process-activity sampling, `pane close` for ended agents) take the "no answer" branch here. See "What I could not reproduce".

Machine and versions: Linux 6.1.0-48-amd64 x86_64, QEMU Virtual CPU 2.5+, 8 vCPUs, 16 GB, Node.js v24.6.0, SQLite 3.50.4 (the one inside `node:sqlite`), git 2.39.5, `CLK_TCK` 100. Baseline build: commit 6881413 (v0.2.0), compiled into `/tmp/capstan-prof/dist-base`; after build: this branch compiled into `dist-new`.

Commands (all in `docs/research/daemon-profile/`, raw output under `results/`):

```sh
setup.sh <live controller.sqlite> <checkout> /tmp/capstan-prof      # copy, shims, scratch credential
reset.sh                                                            # pristine copy back, run files removed
start.sh <dist> [node flags]                                        # scratch daemon under env -i
profile-run.sh <label> <dist> idle|load 10                          # --cpu-prof (main thread) + heap snapshot, 10 min
mem-run.sh <label> <dist> idle|load 10                              # memory after 10 min, no profiler
protocol.sh <label> <dist>                                          # status x200, CPU 3x60 s idle, 3x60 s under the mix, memory at 10 min
stmts.mjs <dist> <root>                                             # statements and SQL time per operation, in process
alloc.mjs <dist> [calls]                                            # bytes allocated per function over N status calls
top.mjs <file.cpuprofile>   heap.mjs <file.heapsnapshot>            # summaries
bench.mjs latency|replay|cpu|mem                                    # the load generator and /proc sampler
```

The replayed mix is `status` at 0.9/s by the operator. From the last 20000 lines of the live `daemon.log` (26356 s): `status` by the operator 16621 calls (0.63/s, 171 ms mean), then a handful of Supervisor/PM `inbox`, `observe`, `ack`, `wait` calls (about 0.01/s together, 2 to 66000 ms each because `wait` blocks). Those are too rare to move CPU, so the mix is the operator's `status` only, at the 0.9/s the task gives.

## Baseline profile

### CPU

Two 10-minute `--cpu-prof` runs (main thread) per case, taken at different times on the shared machine. "Busy" is every sample that is not `(idle)`; shares are of main-thread self time inside busy. Owner is the first of these frames on the stack: `status` handler, report relay `tick` (`src/reports.ts`), driver `#tick` (`src/driver.ts`), startup recovery.

| Run (10 min)                         | Process CPU (/proc) | Main thread busy | status | report relay tick | driver tick | startup recovery | rest |
| ------------------------------------ | ------------------- | ---------------- | ------ | ----------------- | ----------- | ---------------- | ---- |
| baseline idle, round 1               | 17.7%               | 42.9% of wall    | 0%     | 44.4%             | 51.3%       | 1.7%             | 2.6% |
| baseline idle, round 2               | 21.1%               | 16.7% of wall    | 0%     | 46.5%             | 49.1%       | 2.7%             | 1.7% |
| baseline under status 0.9/s, round 1 | 27.4%               | 56.0% of wall    | 28.7%  | 30.7%             | 35.1%       | 2.8%             | 2.7% |
| baseline under status 0.9/s, round 2 | 33.2%               | 28.3% of wall    | 40.0%  | 27.6%             | 29.0%       | 1.5%             | 1.9% |

The live 43% is a figure whose method I do not have; the closest numbers on the copy are round 1's 42.9% busy main thread and the 16 to 33% of process CPU. "Busy" is wall time in non-idle samples, so it includes time the thread waited for a CPU on a loaded machine, which is why round 1 shows 43% busy for 17.7% of process CPU. Unprofiled, the protocol measured 16.1% idle and 27.3% under the mix (below).

Of the busy time about 96% is self time inside `node:sqlite` (`all` 7.9% of wall, `get` 6.5%, the row-copy wrapper 0.9%, plan-notice `sent` 0.8%, of 16.7% busy in round 2 idle; 38.8 of 42.9 points in round 1): the JavaScript around the queries is negligible. The attribution below is therefore by statement, taken by counting and timing every `Statement.get/all/run` while one pass of each loop runs on the copy (`stmts.mjs`, `results/stmts-old.txt`, `results/stmts-new.txt`).

#### Report relay tick (every 2 s): 146.1 ms, 50 statements, steady state

| Cost                                                                                                                                                      | ms per tick | Share of the loop |
| --------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- | ----------------- |
| `unannouncedPlanNotices`: 40 x `SELECT 1 FROM messages WHERE substr(body,1,?) = ?` (one per approved plan and per signoff; each reads every message body) | 135.0       | 92.4%             |
| `unannouncedReviews`: `SELECT * FROM reviews WHERE state IN (...) AND notified_message_id IS NULL ORDER BY sequence` (walks all 287 reviews, none match)  | 4.3         | 2.9%              |
| `listAgents()` only to count active PMs                                                                                                                   | 3.4         | 2.3%              |
| everything else (actors, reports, plans, signoffs, findings)                                                                                              | 3.4         | 2.4%              |

#### Delivery driver tick (every 2 s), ledger part: 160.9 ms, 45 statements, steady state

| Cost                                                                                                                    | ms per tick | Share |
| ----------------------------------------------------------------------------------------------------------------------- | ----------- | ----- |
| `messagesFor(PM)` x 2 (`#judgeWakes` and `#judgeStaleFor`): `SELECT *` of all 1046 PM messages with bodies, each time   | 63.8        | 39.7% |
| `advanceMessaging` / `#evaluateMessaging`: scans all messages by state (`NOT IN` the three final states)                | 44.4        | 27.6% |
| `queueMissingDeliveryNotices` `missing()`: `m.state IN ('unacked','expired','failed')` joined to agents, scans messages | 43.3        | 26.9% |
| `listAgents()`, then filtered to active in JavaScript                                                                   | 3.1         | 1.9%  |
| agent waits, state history, pauses (per active agent), wakes                                                            | 4.2         | 2.6%  |
| rest                                                                                                                    | 1.9         | 1.2%  |

The first driver pass after a start also applies the transitions that are due on the copy (224.7 ms, 96 statements), once. The real daemon's profile agrees with the statement timing: in round 2 idle `unannouncedPlanNotices` > `sent` is 43.7% of busy main-thread time (7.3% of wall), `#advance` 29% (4.9% of wall), `messagesFor` through `#pmMessages` 18.6% (3.1%), of which `#judgeWakes` 9.6% and `#judgeStale` 9.0%, and `queueMissingDeliveryNotices` > `missing` 13.8% (2.3%).

#### `status`: 128.2 ms SQL of 137.9 ms, 307 statements

| Cost                                                                                                                                    | ms       | Share of the handler |
| --------------------------------------------------------------------------------------------------------------------------------------- | -------- | -------------------- |
| PM mail summary: `messagesFor(PM)` loads all 1046 messages to count the pending ones                                                    | 33.0     | 23.9%                |
| plan package progress: 72 x `SELECT r.report_id FROM agent_reports ...` (planner walks `agent_reports` newest first)                    | 27.7     | 20.1%                |
| 6 x `SELECT state FROM reviews WHERE subject_report_id = ?` (planner walks reviews by sequence)                                         | 19.3     | 14.0%                |
| `unresolvedMessages`: `state NOT IN (...)` scans all messages                                                                           | 11.6     | 8.4%                 |
| `supervisionReason`: `MAX(sequence) FROM controller_events WHERE entity_type = ... AND to_state = ...` (23336 events, primary key scan) | 10.1     | 7.3%                 |
| `listAgents()` called twice                                                                                                             | 9.9      | 7.2%                 |
| 68 x integration membership, 12 x authenticate, 20 x integration reports, 25 x plan packages, and the rest                              | about 17 | 12.3%                |
| JavaScript outside SQL (building, JSON)                                                                                                 | 9.7      | 7.0%                 |

The live 12 points of `status` (about 0.9 calls/s x 137 ms = 12.4% of one core) is exactly this handler. The other live 31 points are the two loops above plus startup and Herdr work.

### Memory

`RssAnon` after 10 minutes, with `--import memreport.mjs` writing `process.memoryUsage()`, V8 heap-space statistics and the anonymous mappings of `/proc/self/smaps`, then a heap snapshot (`results/*/memreport.json`, `heap.txt`):

| Baseline run (10 min) | RssAnon  | V8 heap committed | of which new space | old space (used) | outside the V8 heap |
| --------------------- | -------- | ----------------- | ------------------ | ---------------- | ------------------- |
| idle                  | 187.7 MB | 147.6 MB          | 128 MB (27.5 used) | 15.8 MB (11.0)   | 36.2 MB             |
| under status 0.9/s    | 126.0 MB | 84.9 MB           | 64 MB (21.6 used)  | 16.5 MB (10.4)   | 38.6 MB             |

Round 1 on the same copy measured 177.0 MB idle and 182.0 MB under the mix, and unprofiled 10-minute runs 163.3 MB idle and 101.5 MB under the mix (memory split in `results/*-mem/memreport.json`), so a single baseline reading can differ by 80 MB; the cause is the allocation volume below.

- **The baseline's anonymous memory is the V8 young generation, not retained data.** The live heap after a full GC is 11.8 MB (snapshot `heap.txt`): 4.5 MB strings (module sources and SQL texts), 4.2 MB compiled code, 1.0 MB arrays, 0.7 MB object shapes, 0.5 MB closures; no data structure holds ledger rows. New space is 64 to 128 MB because the two loops materialise thousands of row objects per tick (3329 rows per driver pass, of which 3270 are PM messages with about 0.9 MB of bodies per read; 662 rows per relay pass; 2695 rows per `status`), which makes V8 grow the semi-spaces and keep them grown. That is 70% (idle) and 52% (under the mix) of `RssAnon`. A sampling heap profile of 100 in-process `status` calls (`alloc.mjs`, `results/alloc-old.txt`) shows 8.6 MB allocated per call before the fixes, 4.2 MB of it in `node:sqlite` building rows and 3.2 MB in the adapter's `{ ...row }` copy of every row; after the fixes 1.6 MB per call (`results/alloc-new.txt`).
- The remaining 36 to 39 MB outside the V8 heap is the same in every run: malloc arenas (22 MB and 10 MB mappings in the idle run), thread stacks, SQLite's page cache and prepared statements, and Node's own structures; I did not split it further. Nothing in the daemon caches rows: the 256-entry statement cache holds prepared statements only.
- The live 320 MB RSS (258 MB anonymous) is 65 to 75% explained by the 219 to 242 MB RSS (166 to 188 MB anonymous) of the copy. The remaining 60 to 90 MB of anonymous memory is **not explained**: the live daemon also holds real Herdr answers (pane JSON, screen text per agent per tick), the process-activity samples of working agents, and `cstan-dash` and `hub status --watch` connections, none of which exist on the copy.

## What was fixed

All changes keep every result, wire format and CLI output identical (parity test below).

1. **Report relay, plan notices** (`src/controller/plans.ts`): a notice found in `messages` is remembered in memory (messages are never deleted or rewritten, so a found notice stays found). Only missing notices are looked up again. Steady state goes from 40 body scans per tick to none.
2. **Open messages only** (`src/controller/messages.ts`, `message-notices.ts`, `driver.ts`, `commands/status.ts`): `openMessagesFor(agentId)` and the `unresolved`, `evaluateMessaging` and `missing()` queries read the six non-final states through the new `messages_by_state` index instead of scanning all messages (`state IN ('queued','deferred','sent','unacked','expired','failed')` is exactly `NOT IN` the three final states, by the table's CHECK). `#judgeWakes` does nothing while no wake is outstanding; `#isHead`, `#deliver`, `#judgeStaleFor` and the status PM-mail summary read open messages (the summary and the queue head only ever look at non-final ones; `#judgeWakes` still reads all messages when a wake is pending, because it also looks at what was acknowledged after the wake).
3. **Active agents** (`listAgents()` then filter): `activeAgents()` selects `state = 'active'` in SQL for the driver tick, the report relay, supervision, the launcher and `@pm` routing; `status` calls `listAgents()` once instead of twice.
4. **Migration 0036**, approved by the PM and the user exactly as proposed (`CREATE INDEX IF NOT EXISTS` only; the backup before v36 and all older ledgers behave as before):

   ```sql
   CREATE INDEX IF NOT EXISTS messages_by_state ON messages(project_id, state, recipient_agent_id, sequence);
   CREATE INDEX IF NOT EXISTS agent_reports_by_agent_state ON agent_reports(project_id, agent_id, state, sequence);
   CREATE INDEX IF NOT EXISTS reviews_by_subject_report ON reviews(project_id, subject_report_id, sequence);
   CREATE INDEX IF NOT EXISTS controller_events_by_entity_state ON controller_events(project_id, entity_type, to_state, sequence);
   ```

   Measured on the copy: package progress 23 ms to 3 ms per 62 calls, reviews 2.3 ms to 0.04 ms per call, `supervisionReason` 10 ms to 0.02 ms, pending-message scans 45 ms to under 1 ms. `messages_by_state` is used with `INDEXED BY`: without table statistics SQLite plans these queries as full scans of the primary key (EXPLAIN QUERY PLAN: `SEARCH messages USING PRIMARY KEY (project_id=?)`), and the hint is the only way to make the plan stable. The other three indexes are picked by the planner without a hint; the daemon-cost test asserts all four plans.

5. **Supervision activity** (`findings.ts`): `supervisionActivity()` asks first whether any supervision check exists; with none, the join that walks all messages newest first is skipped. (A `CROSS JOIN` that drives from the checks was tried and rejected: 19 ms with the live 859 checks.)

6. **Rows without a copy** (`src/controller/sqlite.ts`): `Statement.get/all` returned `{ ...row }` copies of the null-prototype rows `node:sqlite` builds. They now give the row that was just built `Object.prototype`, which is what the copy was for. This takes a `status` call from 2.2 to 1.6 MB of allocation (sampled, in process; it changes its CPU by less than the noise); callers still see plain objects.

No poll cadence or wording changed.

## Baseline versus after

Same copy, same commands (`protocol.sh`, no profiler attached, fresh copy each time). `status` is 200 sequential calls after 5 warm-ups and 75 s of daemon start-up churn; CPU is `utime+stime` from `/proc/<pid>/stat` over 60 s windows; the mix is `status` at 0.9/s; memory is read 600 s after the start, the last 5 minutes of it under the mix.

| Measure                                 | Baseline                      | After                     | Change     |
| --------------------------------------- | ----------------------------- | ------------------------- | ---------- |
| `status` p50 / p95 / max over 200 calls | 134.5 / 473.1 / 493.0 ms      | 23.8 / 28.2 / 34.4 ms     | 5.7x / 17x |
| CPU idle, 3 x 60 s                      | 16.1, 15.7, 16.5% (mean 16.1) | 0.7, 0.8, 0.7% (mean 0.7) | -95%       |
| CPU under the mix, 3 x 60 s             | 29.1, 26.2, 26.7% (mean 27.3) | 3.2, 3.0, 3.1% (mean 3.1) | -89%       |
| `status` p50 / p95 during the mix       | 280.1 / 981.5 ms (see note)   | 25.8 / 32.3 ms            |            |
| VmRSS at 10 min, under the mix          | 219.0 MB                      | 105.0 MB                  | -52%       |
| RssAnon at 10 min, under the mix        | 165.8 MB                      | 51.8 MB                   | -69%       |

Memory after 10 minutes in separate runs with no profiler (`mem-run.sh`), the same copy and scripts:

| Run           | Baseline RssAnon | After RssAnon | Change | After: V8 new space (used) / outside the V8 heap |
| ------------- | ---------------- | ------------- | ------ | ------------------------------------------------ |
| idle          | 163.3 MB         | 37.9 MB       | -77%   | 8 MB (1.9) / 18.7 MB                             |
| under the mix | 101.5 MB         | 69.4 MB       | -32%   | 32 MB (11.0) / 15.5 MB                           |

The baseline under the mix has been 101 to 182 MB across the five loaded runs I took (182.0, 168.8, 165.8, 126.0, 101.5; V8 sizes the young generation at 64 MB or 128 MB depending on how its first minutes went). The fixed daemon under the mix measured 51.8 and 69.4 MB unprofiled (89.6 MB profiled, with the earlier build). Against the lowest baseline (101.5 MB) that is -49% and -32%; against the median baseline (165.8 MB) -69% and -58%. The 40% target is therefore met when the baseline sits where it usually did and missed in the one run where it did not. Cutting it robustly needs a cap on the young generation (`--max-semi-space-size` at daemon spawn, which lives in `src/client.ts`, outside this package) or a smaller `status`.

Profiled 10-minute runs (`profile-run.sh`; the profiler itself costs CPU, so these are higher than the unprofiled numbers but comparable with each other):

| Run                     | Process CPU | `status` p50 / p95 under the mix | RssAnon  | Main thread busy |
| ----------------------- | ----------- | -------------------------------- | -------- | ---------------- |
| baseline idle           | 21.1%       |                                  | 187.7 MB | 16.7% of wall    |
| after idle (*)          | 5.3%        |                                  | 65.6 MB  | 2.4% of wall     |
| baseline under the mix  | 33.2%       | 136.4 / 154.9 ms                 | 126.0 MB | 28.3% of wall    |
| after under the mix (*) | 6.6%        | 39.8 / 197.2 ms                  | 89.6 MB  | 10.9% of wall    |

(*) Taken with the build before the last change (the adapter no longer copies rows, 2.2 to 1.6 MB per `status` call); the unprofiled tables above are with it. The profiler adds CPU and heap, which is why these rows are higher than the unprofiled ones.

The baseline mix row of the protocol (280 / 981 ms) came from a busier moment on the shared machine: an earlier protocol repeat (not kept) measured 130.3 / 143.2 ms for the same replay, and the profiled run 136.4 / 154.9 ms. The profiled "after" run's p95 of 197 ms is the same effect on a loaded machine, not a regression: the unprofiled runs' p95 is 28 to 33 ms.

### Statements and SQL time per operation on the copy (`stmts.mjs`, in process, one pass)

| Operation                             | Baseline (wall, statements) | After (wall, statements)                                   |
| ------------------------------------- | --------------------------- | ---------------------------------------------------------- |
| report relay tick, steady state       | 146.1 ms, 50                | 7.8 ms, 10                                                 |
| report relay tick, first pass         | 157.6 ms, 50                | 163.6 ms, 50 (once after a start; fills the notice memory) |
| driver tick ledger part, steady state | 160.9 ms, 45                | 5.3 ms, 44                                                 |
| driver tick ledger part, first pass   | 224.7 ms, 96                | 19.2 ms, 95                                                |
| `status` over the socket              | 136.2 ms, 307               | 28.0 ms, 307 (21 to 37 ms across repeats on this machine)  |

## Result parity and the merge gate

- `test/fixtures/daemon-cost/generate.mjs` builds, deterministically and with no live data, a ledger of the live shape: 500 agents (493 ended), a PM with 1046 messages of about 800 bytes, 2300 other messages, 23 plans with revisions, packages, signoffs and notices, 200 reports, 287 reviews, 30 integrations with 150 members, 20000 events, 40 supervision checks, a degraded supervision control row.
- `export.mjs` (with `capture.mjs`) captures, through the real command handlers over a socket with a fixed clock: `status`, `inbox` for three agents, `inspect` for four ids, and the outputs of the loops (`unannouncedPlanNotices`, `unannouncedReports`, `unannouncedReviews`, the PM's open mail and its summary, `unresolvedMessages`, `advanceMessaging`, `queueMissingDeliveryNotices`). `test/fixtures/daemon-cost/golden.json` was exported with the code before the fixes (`/tmp/capstan-prof/dist-base`, commit 6881413). Paths and the pid are normalised; the clock is fixed so relative fields are exact.
- `test/daemon-cost.test.ts` regenerates the ledger with the current build and asserts the capture is deep-equal to the golden, then asserts a budget for `status` (<= 260 statements, <= 1600 rows, < 1.5 s), the report relay steady state (<= 16 statements, <= 300 rows) and the driver ledger part (<= 80 statements, <= 400 rows). Measured now: status 203 statements and 1108 rows (22 ms), relay 12 statements and 125 rows, driver 45 statements and 60 rows. The budgets do not hold on the pre-fix code: `CAPSTAN_DAEMON_COST_DIST=/tmp/capstan-prof/dist-base node --test dist/test/daemon-cost.test.js` fails on the status row budget (the pre-fix numbers on the same ledger are status 203 statements, 2696 rows, 98 to 166 ms; relay 56 statements and 662 rows; driver 46 statements and 2239 rows). A second test asserts the four `EXPLAIN QUERY PLAN`s use the new indexes.
- Results with and without the migration: the golden is the result of the code and schema before 0036 (v35, no new index, original SQL); the current code on the v36 ledger returns the identical document, so results are equal with and without the indexes.
- `test/ledger-compat.test.ts` now takes the copy with `copy-ledger.mjs`, which folds the WAL, so the header it compares is the ledger's own and bytes 24 to 27 change only when a migration ran. A new case builds a scratch ledger (`CAPSTAN_LIVE_LEDGER_ROOT` pointing at it) whose writer stays open with `wal_autocheckpoint = 0` and uncheckpointed frames; it fails with the previous copy logic (3 of 3 runs) and passes now, 30 consecutive runs passed.

## What I could not reproduce or verify

- **Herdr-dependent costs.** The copy's driver finds no panes (the shim answers nothing), so per-agent `pane get`, process sampling, `pane close` retries for ended agents, and the Herdr JSON parsing do not run. They are `child_process.spawn` per active agent per tick; in the live profile they are part of the unexplained 31 points. On the copy, `spawn` and `spawnSync` are 0.2 to 0.7% of busy time. The fixes above do not touch them; with the SQL cost gone they are now the largest remaining item of the idle daemon (0.8% CPU).
- **The live 43% and 258 MB anonymous are only partly reproduced**: 16 to 21% process CPU idle and 27 to 33% under the mix, 166 to 188 MB anonymous, against 43% and 258 MB live. The rest is live-only (see Memory). I cannot attribute it without profiling the live daemon, which is out of bounds.
- **Machine noise.** Other agents' load on the machine. Round 1 and round 2 differ by up to 2.5x in "busy wall" share and 50 MB in RssAnon; the unprofiled protocol repeated within 1 point on CPU.
- **Young-generation sizing is bistable.** After the fixes the idle daemon keeps the new space small (1 to 8 MB); under `status` at 0.9/s V8 grows it to 16 MB or 32 MB (51.8 MB RssAnon in the protocol run, 69.4 MB in the second run, 89.6 MB profiled). A 3-minute run with only the row-copy change had 16 MB and 54.4 MB (`results/exp-adapter-3min`); it was 32 MB at 10 minutes. I did not change V8 flags, because the place to set them is outside this package.
- `status` takes 18 to 24 ms in process and 24 to 27 ms over the socket in the daemon with its loops (the p50 of 23.8 ms is 6 ms under the 30 ms target, the earlier build measured 27.4 ms). The remaining cost is spread over 307 small statements (package progress 72, integration membership 68, authenticate 12) and building 124 KB of JSON; no single item is above 5 ms.
- The first pass of the report relay after a restart still pays one scan per notice (205 ms once). Plan notices missing from the ledger are looked up on each tick until announced; the relay announces them in the same tick, so this does not repeat in practice.
