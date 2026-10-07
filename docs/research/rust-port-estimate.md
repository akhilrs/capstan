# Porting Capstan from TypeScript/Node to Rust: estimate

Requirement `req-rust-estimate` (Nexora PM-132). Date: 2026-10-07. Machine: the workstation behind `rust-dash-performance.md`
(8 cores, Linux 6.1.0-48-amd64, Node 24.6.0 with SQLite 3.50.4, rustc/cargo 1.99.0). Code measured: `6881413` (v0.2.0).

This is an estimate only. No source code was changed. It builds on what plan-22 established for `cstan-dash` (`dash/`): the pinned
stable toolchain (`dash/rust-toolchain.toml`), exact crate pins (`=x.y.z` in `dash/Cargo.toml`), the release profile (`lto`,
`codegen-units = 1`, `strip`), musl targets, and parity proven by a Node exporter writing JSON fixtures that the Rust tests replay
(`dash/tests/parity/`, `test/dash-parity-export.ts`).

**Short answer.** The per-invocation cost of the `cstan` CLI (170 to 220 ms, 66 to 73 MB per call, and about 65 MB held by every
blocked `cstan wait`) is almost entirely Node runtime and module loading, and Rust removes it. The live daemon's cost (43% of one
core, 320 MB) is mostly our own work on a large ledger; Rust would reduce it but would not fix a query that scales with data, and the
half of it that I could attribute is the `status` handler. A full port is about **950 hours likely** (620 to 1,470). I recommend
option (c) now, a Rust thin client plus a Node-side profile and fix of the daemon (about **164 hours**), structured as the first
step of the incremental port (b), and deciding on the daemon port with the profile in hand.

## 1. Inventory

Measured with `wc -l` over the files of each area (no `cloc` or `tokei` on this machine; `wc -l` counts every line, comments and
blank lines included). The command was a shell function `area(){ n=$1; shift; printf "%-18s files=%3d loc=%6d\n" "$n" "$#" "$(cat "$@" | wc -l)"; }` (run by `sh`)
called once per row with the file list shown. The ten `src/` rows add up to `find src -type f | wc -l` = **126 files** and
`find src -type f -exec cat {} + | wc -l` = **45,844 lines**.

| Area | Files (what is counted) | Files | Lines | External dependencies |
|---|---|---:|---:|---|
| Controller daemon | `src/daemon.ts driver.ts supervision.ts notifier.ts watch.ts observe.ts control.ts context.ts command-runner.ts commands.ts reports.ts reviews.ts pm-mail.ts restart.ts restart-helper.ts seed.ts` | 16 | 5,154 | `node:net` (Unix socket server), `node:fs`, `node:child_process` (restart helper, notifier), `node:crypto` |
| Kernel / ledger | `src/controller/*.ts` except `sqlite.ts`, `database.ts` | 31 | 17,220 | the SQLite layer below; `node:crypto` (ids, hashes) |
| SQLite layer | `src/controller/sqlite.ts database.ts` | 2 | 656 | `node:sqlite` (`DatabaseSync`, `backup`; replaced `better-sqlite3` and `fs-ext`), WAL, exclusive SQLite lock on `controller.lock` (`ownership.ts`) |
| Migrations | `migrations/*.sql` (0001 to 0035) | 35 | 2,016 | plain SQL; sha256 of the file bytes is stored in `schema_migrations.checksum` |
| `cstan` CLI and commands | `src/cli.ts client.ts commands/*.ts text.ts task-text.ts sea.ts` | 17 | 5,115 | `node:net` (client), `node:sea`; opens the ledger directly for offline `status`/`inspect` and `config sync` |
| Config | `src/config/*.ts` | 7 | 2,119 | `smol-toml` 1.8.0 (`capstan.toml`) |
| Prompts | `src/prompts.ts roles/designer-prompt.ts conventions.ts researcher-policy.ts` | 4 | 979 | none (text; golden files in `test/fixtures/prompts-off/`) |
| Launcher / Herdr integration | `src/launcher/*.ts launcher.ts herdr/*.ts layout.ts` | 23 | 6,537 | `herdr` CLI (spawned), `git` (worktrees), `/proc` and `ps` (process activity), ANSI screen parsing |
| Operator | `src/operator.ts operator-policy.ts` | 2 | 1,196 | the kernel; operator key file |
| Plans / integrate | `src/plans.ts integration.ts git.ts git-requirement.ts nexora.ts` | 5 | 2,077 | `git` CLI (merge, branch, inspect) |
| Node dashboard (already ported) | `src/dash/*` | 19 | 4,791 | `ink` 7.1.1, `react` 19.3.0; superseded by `cstan-dash` (plan-22) |
| **Source total** | | **126** | **45,844** | |
| Packaging / installer / release | `scripts/*` and `install.sh` | 13 | 2,440 | `esbuild`, `postject`, Node SEA, nodejs.org downloads, `tar`/`xz`, cargo (dash) |
| Tests | `test/*.ts test/*.tsx` | 119 | 62,029 | `node:test`, `ink-testing-library`; 1,625 tests in the last `npm run check` run; fixtures in `test/fixtures`, `test/golden` (69 files) |
| For reference: Rust dash | `dash/src/**/*.rs` / `dash/tests/**/*.rs` | 39 / 7 | 8,189 / 3,297 | `ratatui`, `crossterm`, `serde_json`, `indexmap`, `signal-hook`, `libc` |

Calibration from plan-22: the Node dashboard's 4,791 lines became 8,189 lines of Rust plus 3,297 lines of Rust tests (1.7 times
the source lines, plus tests). The hours plan-22 actually took are not recorded in the repository (squash commits only), so the
estimates in section 3 are engineering judgement per package, not a rate fitted to plan-22.

Rust crates a port would add (to be pinned exactly, as in `dash/Cargo.toml`): `rusqlite` with bundled SQLite (replaces `node:sqlite`),
`toml` (replaces `smol-toml`), `sha2`, `uuid` or `getrandom`, `serde`/`serde_json` (already used), and either `tokio` or a
`mio`/thread-per-connection design for the socket server and long-poll `wait`.

## 2. Measured baseline

CPU is percent of **one core** from `/proc/<pid>/stat` (utime + stime, `CLK_TCK` = 100) over the window; RSS is `VmRSS` from
`/proc/<pid>/status`. The sampler was a 10-line `sh` script: read `$14+$15` of `/proc/<pid>/stat`, `sleep 60`, read again,
`(after-before)*100/CLK_TCK/60`.

### 2.1 Live daemons (read-only from `/proc`, nothing sent to them)

| Daemon | Command used | CPU (3 x 60 s, 10:03:55 to 10:06:55) | RSS |
|---|---|---|---|
| Capstan project, pid 14540 (`~/.local/share/capstan/current/bin/cstan daemon`, the v0.2.0 SEA binary; started at 09:50 after migrations 0034/0035) | `/proc` sampler above | **43.1%, 42.7%, 43.0%** | **320.5 MB** (`VmRSS` 320,540 to 320,572 kB). An earlier read: `VmRSS` 312,112 kB = `RssAnon` 258,316 kB + `RssFile` 53,796 kB |
| Another project (nexora), pid 2433 (`node dist/src/cli.js daemon`, up 2.4 days) | same | 14.5%, 14.4%, 14.4% (lifetime average 12.4%: 2,616,334 ticks over 2.45 days) | 298 MB |
| A third project, pid 20944 | same | 0.5%, 0.6%, 0.6% | 75.6 MB |

Where the live Capstan daemon's CPU goes:

* **Thread split** (per-thread `/proc/14540/task/*/stat` over 30 s): main thread **43.3%**; the four `V8Worker` threads (GC helpers)
  0.1% each; the four `libuv-worker` threads 0.0 to 0.1%. The cost is JavaScript on the main thread, not background GC threads or
  thread-pool I/O.
* **Request log** (the daemon's own `.capstan/daemon.log`, read with `tail -c` and aggregated by a Python script over the last
  600 s; it records each request's command and handler `ms`): 559 requests. `status`: 540 requests (0.90 per second), average
  **135 ms**, max 210 ms, 73.0 s in total = **12.2% of one core**. `wait`: 7 long polls averaging 90 s each (blocked, not CPU).
  `observe` 3 x 86 ms; `ack` and `inbox` under 7 ms each.
* **Not attributed: about 31 points of the 43%.** It is outside logged request handlers (background loops such as supervision, the
  delivery driver, message notices and the process-activity probe, plus response serialization and GC on the main thread). Not
  measured: splitting it needs a `--cpu-prof` of a daemon running on a copy of the live ledger, and copying the live ledger was not
  permitted in this session (see section 5).

### 2.2 Scratch daemon (`/tmp/cr/p`, fresh `cstan init`, `CAPSTAN_SOCKET`/`CAPSTAN_TOKEN` unset)

Run with `env -i HOME=$HOME PATH=/tmp/cr/bin:/bin` so `herdr` was not on `PATH`, and a `git` shim that refused any path outside
`/tmp` (it refused nothing). Started with `node dist/src/cli.js start` from the worktree build; stopped with `cstan stop` in the
same directory.

| Measurement | Command | Result |
|---|---|---|
| Idle CPU, 60 s | `/proc` sampler | **0.08%** |
| Idle RSS | `/proc/<pid>/status` | **83.2 MB** (`RssAnon` 28.8 MB, `RssFile` 54.4 MB, `VmHWM` 83.7 MB), 11 threads |
| Daemon CPU per `status` request | 100 x `cstan status --json` (SEA client), daemon ticks before/after | **2.0 ms** per request (200 ms total); RSS unchanged |

The same handler takes 2 ms here and 135 ms on the live ledger (66.7 MB `controller.sqlite`, size from `ls -la`). The difference
scales with ledger data, so it is query, row mapping and JSON work, not Node startup. `status-query-timings.md` measured 13.2 ms p50
on a 450-agent ledger with no messages or reports, which points at the message/report volume.

### 2.3 One `cstan` CLI invocation (scratch project, 20 runs each)

Command: `env -i HOME=$HOME PATH=/tmp/cr/bin:/bin /usr/bin/time -f "%e %M" <command>` in `/tmp/cr/p` with the scratch daemon running;
median, min and max of wall time and of max RSS. `/usr/bin/time` has a 10 ms resolution.

| Command | Wall median (min to max) | Max RSS median (max) |
|---|---|---|
| `node -e 0` (Node floor) | 0.030 s (0.020 to 0.040) | 45.4 MB (48.4) |
| `node dist/src/cli.js ping` (npm build) | **0.220 s** (0.210 to 0.240) | **72.3 MB** (74.9) |
| `node dist/src/cli.js status --json` | 0.220 s (0.210 to 0.250) | 72.6 MB (74.6) |
| `node dist/src/cli.js inbox` | 0.210 s (0.210 to 0.240) | 73.3 MB (76.2) |
| SEA binary `cstan ping` (installed v0.2.0) | **0.170 s** (0.160 to 0.170) | **66.3 MB** (68.3) |
| SEA binary `cstan status --json` | 0.170 s (0.160 to 0.180) | 66.3 MB (68.3) |
| `cstan-dash --version` (Rust, for scale) | 0.000 s (below resolution) | 0.6 MB |
| Import `ink` + `react` only (not loaded by the commands above) | 0.540 s | 102.3 MB |

A `node --cpu-prof --cpu-prof-interval 100` of one `ping` (239 ms sampled) put **105 ms (44%)** in the ESM loader (module
resolution, `package.json` lookups, stats; the npm build loads 126 modules), 14 ms in Node bootstrap, 7 ms in GC, 19 ms evaluating
`src/dash/format.js` at import, and under 5 ms in `cli.js` itself. The SEA binary loads one bundled file and is 50 ms faster.

Every agent's blocked `cstan wait` is one of these processes for up to 90 s: a `ps -eo rss,args` read during this work showed 2
`cstan` client processes holding **140 MB** together.

Unrelated observation from the same `ps` read (no action taken): three daemons whose working directory is a deleted `/tmp` test
directory (pids 15243, 3534, 17741; 44, 28 and 12 MB, 0% CPU) and 43 `herdr --session capstan-test-*` servers (208 MB) are still
running from earlier test runs.

### 2.4 Expected gain from Rust, per component

Rust figures are estimates from the measured `cstan-dash` (7 to 8 MB RSS while polling and rendering, 0.6 MB for `--version`), not
measurements of Rust code that does not exist yet.

| Component | Today (measured) | Where the cost is | Expected with Rust | Fixable in Node instead? |
|---|---|---|---|---|
| `cstan` per call | 170 to 220 ms, 66 to 73 MB | Node runtime floor (30 ms, 45 MB) plus module loading (about 105 ms) | about 5 to 15 ms, 2 to 4 MB (socket round trip plus the daemon's handler) | Partly: bundling the npm build as the SEA does saves about 50 ms; the 30 ms / 45 MB floor stays |
| Blocked `cstan wait` | about 65 to 70 MB each, up to 90 s | Node runtime held idle | 2 to 4 MB each | No |
| Daemon idle (fresh ledger) | 0.08% CPU, 83 MB | Node runtime and V8 heap | CPU unchanged (already negligible), about 8 to 15 MB | No |
| Daemon `status` on the live ledger | 135 ms per call, 12.2% of a core at 0.9/s | Scales with ledger data: SQLite query time plus row-to-object mapping and JSON | SQLite query time **unchanged** (same engine); mapping and JSON perhaps 3 to 10 times faster. Net gain unknown until profiled | **Yes, likely first**: migration 0035 cut the `roles` query from 33 ms to 1.7 ms without Rust |
| Daemon unattributed main-thread work | about 31% of a core | Not measured | Unknown; a polling loop that rescans the ledger costs the same SQLite time in Rust | Probably, once profiled |
| Daemon memory on the live ledger | 320 MB (258 MB anonymous) | V8 heap (not split: no heap snapshot of the live daemon was taken) | Likely tens of MB, not measured | Partly (caches, retained objects) |

Honest summary: Rust removes Node's fixed costs (the 45 MB floor, module loading, the V8 heap) everywhere; that is a large, certain
win for the CLI and `wait`, and a large memory win for the daemon. For the daemon's CPU under load, the measured evidence points at
our own data-dependent work, which a port alone does not fix.

## 3. Package breakdown (full port, dependency order)

Hours are developer hours including each package's own Rust unit tests. Parity is proven the plan-22 way: a Node exporter
(`test/*-parity-export.ts`) runs the TypeScript code on fixed inputs and writes JSON fixtures that the Rust tests replay and compare
field for field. Three invariants hold for every package: **existing ledgers open unchanged** (same SQLite file, same
`schema_migrations` rows and checksums, all 35 migrations byte-identical and applied by the same rules), **wire protocol v1 is
unchanged** (newline-delimited JSON frames over the Unix socket, 64 KiB request frame, 1 MiB response, the same error codes and
texts), and **CLI output and exit codes are unchanged** (agents and hooks parse them).

| # | Package | Scope | Depends on | Low | Likely | High | How parity is proven |
|---|---|---|---|---:|---:|---:|---|
| P0 | Workspace and parity harness | Cargo workspace beside `dash/`, shared toolchain and pins, musl x64/arm64 builds, error types, a generic Node exporter for kernel and wire fixtures | none | 16 | 24 | 40 | `npm run check:dash` style gate runs the new crates |
| P1 | SQLite layer and migrations | `rusqlite` (bundled), WAL, the exclusive lock on `controller.lock`, read-only open, migration runner with embedded SQL, sha256 checksums, `pre-vN` backups | P0 | 24 | 40 | 64 | Opens `test/fixtures/ledger-better-sqlite3.sqlite` and ledgers written by Node at every migration version; header bytes and `schema_migrations` compared as `ledger-compat.test.ts` does; Node opens what Rust migrated and the other way round |
| P2 | Kernel / ledger | the 31 files of `src/controller/` (actors, agents, messages, reports, reviews, integrations, plans, findings, operator records, pauses, prompt relay, status) | P1 | 160 | 240 | 360 | Recorded operation sequences: Node applies them to a scratch ledger and exports every response and the resulting rows; Rust replays and compares JSON and rows |
| P3 | Config and prompts | `capstan.toml` parsing and resolution, role and prompt text | P0 | 24 | 36 | 56 | `test/fixtures/prompts-off/*.txt` byte-identical; config fixtures valid and invalid, same error texts |
| P4 | Daemon and wire protocol v1 | socket server, auth, request limits and timeouts, long-poll `wait`, command handlers, supervision, notifier, report/review relay, PM mail | P2, P3 | 80 | 120 | 180 | Wire transcripts recorded from the Node daemon in a scratch project and replayed against the Rust daemon; the Node `cstan` client and the Rust `cstan-dash` run against it unchanged |
| P5 | Launcher and Herdr integration | `herdr` runner, screen and prompt parsing, process activity (`/proc`, `ps`), spawn, release, replace, worktrees | P2, P3 | 72 | 110 | 160 | The ANSI fixtures in `test/fixtures` and `test/fixtures/prompts`; launcher stub tests ported; the existing Herdr live tests run against the Rust binary |
| P6 | Operator and restart | `operator.ts`, `operator-policy.ts`, `restart.ts`, `restart-helper.ts` (operator restart replaces the binary) | P4 | 40 | 64 | 96 | Policy decision fixtures exported from Node; restart smoke in a scratch project |
| P7 | Plans and integrate | plan body parsing, package naming, integration merges, coverage, `git` calls, Nexora links | P2 | 32 | 48 | 72 | Plan bodies and naming exported as fixtures; integration tests on scratch git repositories |
| P8 | `cstan` CLI | argument parsing, usage and error texts, exit codes, client, offline `status`/`inspect`, `init`, `start`, `config`, `dash` handing off to `cstan-dash` | P4 | 48 | 72 | 110 | CLI transcripts (argv, stdout, stderr, exit code) recorded from Node and compared |
| P9 | Packaging, installer, release | drop SEA, `esbuild`, `postject`; cargo musl builds of `cstan` and `cstan-dash`; `install.sh`, `release.mjs`, smoke test | P8 | 24 | 40 | 64 | `smoke-binary.sh` and `test-install.sh` against the new artifacts; a Node-made ledger opens in the Rust binary and back |
| P10 | Test suite port | move the black-box suites (daemon, CLI, recovery, launcher, operator) to drive the Rust binaries; retire tests of Node internals that the per-package Rust tests replace | P4 to P8 | 80 | 120 | 200 | The same scenarios pass against both implementations before the Node ones are deleted |
| P11 | Cutover and soak | shadow run on scratch copies, re-measure section 2, docs, remove the Node source and the Node dashboard | P9, P10 | 24 | 40 | 64 | Section 2 re-measured with the same commands; one week on a real project |
| | **Total** | | | **624** | **954** | **1,466** | |

Sums: low 16+24+160+24+80+72+40+32+48+24+80+24 = 624; likely 24+40+240+36+120+110+64+48+72+40+120+40 = 954; high
40+64+360+56+180+160+96+72+110+64+200+64 = 1,466.

Parallelism: after P1, P2 and P3 run in parallel; after P2, P5 and P7 run beside P4; P6, P8 follow P4. The critical path is
P0, P1, P2, P4, P8, P9, P10, P11: 456 / 696 / 1,082 hours low / likely / high (low 16+24+160+80+48+24+80+24, likely 24+40+240+120+72+40+120+40,
high 40+64+360+180+110+64+200+64).

## 4. Options

| Option | What | Low | Likely | High | Gain delivered | Main risk |
|---|---|---:|---:|---:|---|---|
| (a) Full rewrite | P0 to P11, switch over at the end | 624 | 954 | 1,466 | All of section 2.4, only at the end | Nothing ships until P11; the Node code keeps changing meanwhile (35 migrations so far), so parity is a moving target |
| (b) Incremental port | Same packages, shipped in order: Rust `cstan` client first (wire-only commands, falling back to Node for local ones), then the daemon; Node and Rust coexist | 680 | 1,042 | 1,602 | CLI and `wait` gains after the first step; daemon gains later | Coexistence work: both implementations must honour the same project lock and ledger rules, and packaging ships both for a while |
| (c) Hot paths only | Rust thin client for the wire-only commands; profile and fix the daemon's `status` and background cost in Node | 100 | 164 | 264 | The certain part of 2.4 (CLI, `wait`) plus whatever the profile finds in the daemon | The daemon keeps Node's 83 MB floor and V8 heap |

(b) is the package table plus coexistence work: lock and ledger interoperability tests, and dual packaging while both exist,
40 / 64 / 96; the CLI's fall-back to the Node CLI for local commands, 16 / 24 / 40. Totals: 624+56 = 680, 954+88 = 1,042,
1,466+136 = 1,602.

(c) as packages: C1 wire client crate in the P0 workspace 12 / 20 / 32; C2 Rust `cstan` for the wire-only commands with the CLI
transcript parity of P8, 40 / 64 / 96; C3 packaging both binaries 16 / 24 / 40; C4 black-box parity tests 16 / 24 / 40; C5 profile
(`--cpu-prof` on a ledger copy) and fix the daemon in Node, 16 / 32 / 56. Totals: 100 / 164 / 264. C1 to C4 are reused if (b)
follows (about 130 of the 164 likely hours).

**Recommendation: (c) now, as the first step of (b).** It delivers the gain that is certain and large (a per-call cost of 170 to
220 ms and 66 to 73 MB down to a few ms and MB, and the same for every blocked `wait`) for about a sixth of the full effort, and C5
answers the question this estimate cannot: how much of the daemon's 43% and 320 MB is Node and how much is our queries. If, after
C5, the daemon still costs more than the user accepts, continue with (b) from P1; the full rewrite (a) is not recommended, because it
ships nothing for about 950 hours while the Node code keeps moving.

## 5. Risks and open questions for the user

1. **Permission to profile the live data.** The daemon's main-thread split (31 of 43 points unattributed) and its heap (258 MB)
   need a profiled scratch daemon on a copy of the live ledger. Copying it was refused in this session. May a later package copy
   the ledger into `/tmp` for a profile (never opening or writing the original)?
2. **Ledger compatibility.** Node 24.6 bundles SQLite 3.50.4; the `rusqlite` bundled SQLite must be the same or newer so files
   written by one open in the other. Integers above 2^53 (Node throws), the exact `pre-vN` backup naming and retention, and
   the checksum rule (sha256 of the file bytes) must match. No data migration is planned: the schema and files stay as they are.
3. **Backward compatibility and downgrade.** As long as both use the same 35 migrations, a ledger can go from Rust back to Node.
   From cutover on, new migrations must be written once and applied by both until Node is removed. Is a downgrade path required?
4. **Feature freeze.** The kernel changes often. Should the ported areas freeze during their package, or must every change land
   twice (Node and Rust) until cutover? Double-landing adds roughly 20 to 40% to the affected packages and is not in the totals.
5. **Coexistence safety (option b).** The project lock is an exclusive SQLite lock on `controller.lock`; the Rust daemon must take
   the same lock so a Node and a Rust daemon can never both own a project.
6. **Release and packaging.** The npm package (`bin: cstan`) and the 130 MB SEA binaries would be replaced by musl binaries
   (`cstan-dash` is 1.3 MB today). Does npm distribution need to continue, for example as a wrapper that downloads the binary?
   Which platforms: Linux x64 and arm64 only (as today), or macOS too (`test/fixtures/ps-macos.txt` suggests macOS matters for
   process activity)?
7. **Test suite.** 1,625 Node tests (62,029 lines). The plan keeps the black-box suites and ports unit tests per package; tests of
   Node internals are retired. Is that acceptable, or must every test have a Rust twin?
8. **Output text parity.** Agents read `cstan` output and the prompts. Any wording drift is a behaviour change; P8 and P3 compare
   bytes, which makes small fixes during the port deliberate follow-ups, not silent ones.
9. **Estimate calibration.** Hours are judgement, not fitted: plan-22's actual hours are not in the repository. If Nexora has
   plan-22's time logs, the PM can rescale the table with them.
10. **A flaky check.** `npm run check` on unchanged `6881413` failed one test once, "a copy of the live ledger opens and
    migrates" (`test/ledger-compat.test.ts`): it copies the live ledger and its WAL and compares header bytes 24 to 27 (the file
    change counter), which moved from `0x2d` to `0x2e`. The rerun with this document added passed (1,625 tests: 1,621 pass,
    4 skipped, 0 fail). The test depends on the live file's state at copy time; a Rust port inherits the same check.
