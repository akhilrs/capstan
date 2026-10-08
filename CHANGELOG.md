# Changelog

All notable changes are listed here, newest release first. `npm run release` writes each section from the Conventional Commits since the last tag.

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
