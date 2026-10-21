# Changelog

All notable changes are listed here, newest release first. `scripts/release.sh` writes each section from the Conventional Commits since the last tag.

## 0.8.0 (2026-10-21)

### Features

- Host-switchable roles and a complete init sample config (ece6f19)

### Documentation

- **config:** clarify role name length limit (e567ec2)
- **config:** document the optional docs, security, release, host-compat (38ef26b)
- **readme:** align the hosts row in the config table (df953c8)
## 0.7.0 (2026-10-18)

### Features

- Support pi (pi.dev) as a Capstan host kind (d126c91)
## 0.6.0 (2026-10-18)

### Features

- Timed auto-approve-all window for Operator proposals and worker permission prompts (f06f904)

### Bug fixes

- **cache:** turn incremental compilation off in the rust sccache preset (b4d47e1)
- **config:** fall back to the built-in prompt when prompt_file is missing (9af9ed8)
- **status:** report health: disabled instead of a false degraded for the legacy supervision row, and always give a reason when degraded (722d324)

### Tests

- **cache:** keep a forked child from inheriting a held store lock (7695624)
- **cache:** signal kill tests over a pipe instead of a polled deadline (43d6408)

## 0.5.0 (2026-10-13)

### Features

- cstan init: built-in designer prompt (no roles/ folder) (cf18f8c)
- Time-boxed auto-approve grant for non-destructive Operator (9b4f7c2)
- Design approval gate: designer creates the design in Claude (5c9a12d)
- Warm reviewer per plan with diff-scoped briefs (cf23abd)
- Controller-recorded test runs (cstan verify) so reviewers don't (fbd87b3)
- Plan review: report all findings at once and allow ownership (4e94720)
- Info messages don't wake the PM; bulk ack (54b98a3)
- accept unique id prefixes in commands that take a record id (dfde26d)
- per-role max_active and a [liveness] table (cc9d56b)
- **cache:** shared build cache across agents (a188928)
- Configurable branch and worktree cleanup after integration (4f9abfa)
- **install:** add install-local.sh to build and install a local (cbe128e)
- **cli:** group cstan help from one command table and add a man page (8619bee)

### Bug fixes

- **release:** complete changelog and whole plan titles in squash bodies (42db871)
- cstan inbox and wait fall back when an older daemon rejects (b7bb675)
- kill a panicked verify run's processes regardless of reap (d4370c5)
- **cli:** back off briefly when wait --until gets an early empty answer (42411ce)
- **scripts:** run the release smoke on built binaries; clear shellcheck (707426d)
- **launcher:** replace starts a new agent when the recorded successor (c5290be)

### Documentation

- **site:** refresh the Pages site for the Rust 0.4.1 release (a0ddbc1)

### Tests

- derive version-dependent test expectations from the current build (be8a9a0)

### Chores

- **node:** Rust port phase D: remove the Node implementation (1949445)
## Unreleased

### Features

- **cache:** document the build cache in `docs/reference/build-cache.md` (shared presets, modes and their concurrency trade-off, bounds, `cstan cache`, kill-safety, secrets policy, scope, setup reuse and a Gradle and a Bazel example), add `scripts/measure-build-cache.sh` to time a cold and a warm build of any project, and record the measurements for this repository in `docs/investigations/build-cache-measurement.md`: `shared-build-dir` cut a warm Rust build from 55-70 s to 4-6 s.
- **cleanup:** add `cstan cleanup` (`--dry-run`, `apply <cleanup-id> [<branch>...]`, `decline <cleanup-id>`) and a cleanup step after `cstan integrate confirm`: by `[cleanup] branches` the controller offers the landed branches to the PM in one ACTION NEEDED message (`ask`), deletes them and tells the PM (`auto`), or does nothing (`off`). Only the PM and the operator may call it; it is refused while the run is paused or an integration is running. The PM prompt asks you with a picker before anything is deleted.
- **ledger:** the ledger schema moves to **38** (cleanup offers and their branches). Older binaries refuse a ledger at schema 38; to downgrade, restore the pre-migration backup of the ledger.

## 0.4.1 (2026-10-11)

### Features

- **config:** let the researcher role run in any permission mode (5f72ce5)

### Documentation

- **research:** benchmark Node v0.3.0 against Rust v0.4.0 (72b18a4)
- **readme:** add 0.3 to 0.4 upgrade steps, fix installer sentence (d078fff)

### Tests

- **install:** make the foreign bin-dir case hermetic and assert (7dd6252)
## 0.4.0 (2026-10-10)

### Breaking changes

- **cutover:** Rust port phase 3B: Rust-only release, core suites on Rust (9c12749)

### Features

- **daemon:** Rust port phase 3A: frozen parity corpus, Rust client (ef1403f)
- **cli:** Rust port phase 3B: every cstan command native, Node hand-off removed (5540a87)
- **operator:** Rust daemon port phase 2b: opt-in Rust daemon with Node (be41e82)
- **kernel:** Rust daemon port phase 2a: kernel library with Node parity (d62487b)
- **config:** Port the controller daemon to Rust, phase 1 (978e31d)

### Bug fixes

- **ci:** clean shellcheck findings and make the process-probe test (c620c96)
- **ledger:** retry a contended project lock before reporting it held (b3f0606)
- **daemon:** match Node's role-sync ids, commit-message reasons (81b9f17)
- **launcher:** never close a reused pane id or retry a finished teardown (0b0007a)
- **daemon:** restore NODE_OPTIONS when the added flag is not last (4d1cab3)

### Tests

- **daemon:** replay concurrent steps over the socket and record config (a1b5165)
- **install:** run the release step in a throwaway clone (8b04ed0)

### Build

- **cli:** use cargo zigbuild for musl targets when zig is available (b1d0714)

### CI

- split CI into parallel stage jobs, drop the Node job and the release (eae0275)
## Unreleased

Rust is the only implementation (the first release with it is 0.4.0). `scripts/release.sh` writes the 0.4.0 section from the commits; fold these notes into it.

### Breaking changes

- **daemon:** the Node daemon is gone and `[daemon]` selects nothing. `implementation = "rust"` loads with a warning (also logged by the daemon as `daemon:config_warning`); `implementation = "node"` is a configuration error (exit 3) that says to delete `[daemon]`; an empty `[daemon]` is accepted. The resolved configuration of `cstan config check` has no `daemon` member any more. See `docs/daemon.md` for the exact texts.
- **cli:** `CSTAN_DAEMON=node` is refused (exit 3) and `CSTAN_DAEMON=rust` warns on standard error. `CSTAN_DAEMON_BIN`, `CSTAN_NODE_CLI` and `CSTAN_NODE` are no longer read.
- **launcher:** the per-agent `cstan` wrapper always runs the `cstan` executable (beside `cstan-daemon` when the daemon runs under that name) and passes no variable on. A `cstan-daemon` with no `cstan` beside it fails the launch instead of writing a wrapper that execs the daemon.
- **dev:** `scripts/check.sh` is the single gate and replaces `npm run check`; `npm run check` is lint, format and the frozen Node tests only. `check:rust-daemon` and its suite list are removed. CI requires `scripts/check.sh`.

### Bug fixes

- **operator:** a restart accepts a known-good snapshot taken before the upgrade, which holds `cstan-daemon`; the next successful start saves a new snapshot as `cstan`.
- **ledger:** two `cstan start` commands at once end with one daemon and both succeed (the project lock retries on BUSY).

### Documentation

- `docs/rust-daemon.md` is now `docs/daemon.md`; the README, install and Operator references describe the Rust binaries only, with an "Upgrading from 0.3" section; `docs/soak.md` defines the soak gate before the Node tree is removed; `docs/research/rust-cutover-baseline.md` has the Node and Rust measurements side by side.

## 0.3.0 (2026-10-08)

### Features

- **cli:** native Rust `cstan` front end (`rust/`, crates `wire` and `cstan`). It runs the agent wire-only commands and `inbox --hook` itself and hands every other command to the Node implementation (`cstan-node`) with the same argv, environment and exit code. 285 recorded CLI transcripts, taken from the Node CLI, replay byte-identically in Rust, and 37 black-box scenarios run against both the Node and the Rust client, some under a non-UTC `TZ`. `ping` and `inbox` take about 2 ms and under 1 MB instead of about 200 ms and 70 MB (docs/research/rust-port-estimate.md) (5f19445, dca306d)
- **release:** releases ship `cstan-front-<version>-<platform>` beside the Node SEA binary and `cstan-dash`. A binary install puts the front end at `bin/cstan` and the Node binary at `bin/cstan-node`; the `cstan-<version>-<platform>` asset is still the Node SEA, so a 0.2.0 `install.sh` keeps working, and npm installs stay Node only. `install.sh` drops a front end that does not run or belongs to another release and keeps the SEA as `bin/cstan`; `--no-front` and `--front-binary <file>` are new options (850fe11)
- **daemon:** faster `status`. Migration 0036 adds four indexes (`messages`, `agent_reports`, `reviews`, `controller_events`), and the status queries are cheaper and cached. On a copy of a large live ledger: `status` p50 134.5 to 23.8 ms, daemon CPU under status load 27.3% to 3.1%, idle memory 163 to 38 MB (docs/research/daemon-profile.md) (850fe11)

### Bug fixes

- stop the fallback tests exec'ing stubs while another thread forks (the ETXTBSY race) (850fe11)
- make the ledger-compat live-ledger test stable (850fe11)

### Documentation

- add the Rust port estimate for the whole project (74c0d06)

### Tests

- **install:** make the latest-tag refusal scenarios hermetic and format `copy-ledger.mjs` (bc995ad)

### Upgrading

- The daemon applies migration 0036 on its first start. It only creates indexes and writes a `controller.sqlite.pre-v36-<timestamp>.sqlite` backup first.
- A binary install now holds `bin/cstan` (Rust front end) and `bin/cstan-node`. The operator restart replaces `cstan-node`, not the front end.
- A machine without `cargo` runs `npm run check` with both `CSTAN_SKIP_DASH_CHECK=1` and `CSTAN_SKIP_FRONT_PARITY=1`. `npm run release` needs `cargo` unless given `--no-front`.

## 0.2.0 (2026-10-07)

### Features

- sign off a plan on a merged or confirmed integration (7360c06)
- **dash:** Rust dashboard, daemon status query fix and live-daemon guards (f897095)
- Show active tasks in cstan dash (9105c75)
- Dashboard cleanup and waiting-on-you strip (d587aeb)
- **cli:** require a git repository with a commit; add cstan init --git (43cd2cf)
- **messaging:** Reliable PM message delivery: pull all, unread notice, reliable (edde605)
- Paused golden regenerated (only the supervision header line (ea430ce)
- status/dash now show real supervision state (config enabled (73285e1)
- Git standards: Conventional Commits, SemVer releases, task-based (1977676)
- Untracked all review screenshots and git-ignored (286942c)
- Designer workflow: Claude Design first, design skills, anti-slop (3484432)
- README line 67 restored to amend wording (new commit on top, other (d9d4b51)

### Bug fixes

- collapse duplicate changelog entries in the release notes (37271bd)
- give a daemon start time to answer on a loaded machine (fd07672)
- **dash:** fold multi-line spawn titles; share the task row on short (dced0eb)
- **dash:** findings counter shows the visible range; caption says ended (5519c8a)
- **herdr:** mark restart and operator PM notices action-needed (09349e0)

### Documentation

- add a Dashboard (cstan-dash) install section to the README (d28f8d9)
- document the cstan-dash install and dashboard selection in the (432a7a4)
- measure dash and daemon CPU and memory use (0d610c0)

### Refactoring

- **controller:** Split oversized modules and remove the dormant legacy engine (bb534ae)

### Tests

- **dash:** end-to-end parity, redraw, key and performance checks of the (6c1159f)

### Chores

- remove obsolete plans, gate evidence and scratch files (0309406)
