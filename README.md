<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/capstan-logo-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/capstan-logo-light.svg">
  <img alt="Capstan" src="docs/assets/capstan-logo-light.svg" width="460">
</picture>

**A controller for a small team of AI coding agents. You talk to one PM; it runs the crew.**

![Rust](https://img.shields.io/badge/rust-stable-b7410e?logo=rust&logoColor=white)
![License: MIT](https://img.shields.io/badge/license-MIT-blue)
![Version 0.1.1](https://img.shields.io/badge/version-0.1.1-5f8fa8)
![Status: pre-release](https://img.shields.io/badge/status-pre--release-a3672a)

[Quick start](#quick-start) · [How it works](#how-it-works) · [Configuration](#configuration) · [Commands](#command-reference) · [Safety](#safety-model) · [Docs](docs/reference/)

</div>

## What is Capstan

Capstan turns a handful of interactive coding agents into a delivery team. You describe what you want to one agent, the **PM**. The PM spawns workers (developers, designers, reviewers, testers), hands them tasks, collects verified reports, requests independent reviews and tells you which branch to merge.

Every agent is a real interactive session (Claude Code first; Codex and OMP for workers) running in its own Herdr pane and its own git worktree. A separate **controller daemon** carries every message between agents and records every workflow fact in a SQLite ledger. Completion is never guessed from terminal text, and nothing reaches your main branch unless you merge it.

Everything is driven through one command: `cstan`.

## Features

| Feature                                 | What you get                                                                                                                                                              |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **PM and worker roles**                 | One PM talks to you. Workers are free-named roles of four kinds: PM, Developer, Verifier, Supervisor.                                                                     |
| **A worktree per agent**                | Each worker gets its own pane, worktree and branch (`<type>/<task-id>-<slug>`, for example `feat/PM-47-add-login`), cut from your HEAD.                                   |
| **Delivered, acknowledged messages**    | One FIFO per recipient. Messages wait while a worker is busy and stay blocked until acknowledged; problems reach the PM with the exact `cstan resolve` to run.            |
| **Verified reports**                    | A report names a full commit id; the controller checks it lies on that worker's branch before the PM hears about it.                                                      |
| **Independent reviews**                 | `request-review` starts a fresh Verifier at the reported commit. It answers once, pass or findings, and is ended.                                                         |
| **Integrate, confirm, discard**         | Reviewed reports are squashed onto `integration/<plan-id>-<slug>` without a checkout. You merge; the controller confirms it is in HEAD.                                   |
| **Planned work tiers**                  | An optional Architect splits normal and high-risk work into packages with owned files, dependencies and acceptance criteria. High-risk plans get their own review.        |
| **Supervisor and findings**             | A Supervisor watches active workers and raises findings with evidence, a requested correction and a done-when; unresolved ones escalate.                                  |
| **Operator**                            | An optional role that proposes shell commands or a controller restart. Nothing runs without a hash-bound PM approval, a session grant or time-boxed full auto.            |
| **Prompt relay**                        | When a worker stops at a permission prompt, the PM shows you the exact prompt and types only the answer you pick. An unrecognised blocking dialog is relayed as Esc only. |
| **Researcher with MCP servers**         | An optional web-research role with WebSearch, read-only curl and a headless Playwright browser, writing one sourced report.                                               |
| **Pause and resume**                    | You or the PM hold one agent or the whole run with a reason: messages stay queued, spawn, plan assign, review and integrate are refused, `--interrupt` adds one Esc.      |
| **Replace and lost agents**             | Lost panes are detected and reported. `cstan replace` starts a successor seeded from the ledger on the same branch name, restarted at its last accepted report.           |
| **Defaults per role kind**              | Set a model and permission mode once per kind; a role's own value wins.                                                                                                   |
| **Env pass-through and worktree hooks** | Name extra variables for agents; run a `setup` command in each new worktree and a `teardown` before removal.                                                              |
| **Nexora tracking**                     | The PM can mirror requirements and packages into Nexora; the ledger keeps the links and shows drift.                                                                      |
| **Dashboard**                           | `cstan dash` is a read-only terminal dashboard of agents, pipeline, queue and findings.                                                                                   |

## How it works

```mermaid
flowchart TB
    you(["You"]) <-->|"tasks · answers"| PM["PM agent"]
    PM <-->|"cstan commands · notices"| C{{"Controller daemon<br/>SQLite ledger"}}
    C <-->|"plan"| A["Architect<br/>plans packages"]
    C <-->|"task · report"| D["Developers · Designers<br/>own pane, worktree, branch"]
    C <-->|"review request · verdict"| R["Reviewers · Testers<br/>fresh per review"]
    C <-->|"check · finding"| S["Supervisor<br/>watches workers"]
    C -->|"integrate"| I[("integration branch")]
    I -->|"you merge"| H[("your HEAD")]
```

1. **You ask the PM.** It picks a tier. Small work goes straight to a developer; normal and high-risk work goes to the Architect first, which submits a plan of work packages (high-risk plans are reviewed before they are final).
2. **Workers build.** Each commits on its own branch, never pushes or merges, and runs `cstan report <commit> "<summary>"`. The controller verifies the commit before anyone is told.
3. **Reviews are independent.** A fresh reviewer is spawned at the reported commit. Findings go back to the developer, who amends and reports again.
4. **Integration is mechanical.** Reviewed reports are squashed onto an integration branch without touching your checkout. A conflict leaves nothing behind and becomes a new task.
5. **You merge.** After the integration passes its own review you merge it, and `cstan integrate confirm` tells the controller to clean up.

Meanwhile the Supervisor watches for stuck workers, and the controller reports stalled, blocked and lost agents to the PM (a worker whose test or build is still using CPU is not reported as stalled). Workers read their mail with `cstan inbox`, are told when mail waits, and cannot report with unacknowledged mail. The full step-by-step behaviour is in [How Capstan works](docs/reference/workflow.md).

## Quick start

**Requirements:** Linux (x64 or arm64); `curl` or `wget`; **git, with a repository that has at least one commit** (a hard requirement: every worker gets its own worktree and branch cut from HEAD); Herdr for the panes; and Claude Code (the host the starter config uses). The installer downloads two static binaries, `cstan` and `cstan-dash`. Until release 0.4.0 is published, `install.sh` on `main` installs nothing and names the installer of the latest published tag instead (see [docs/reference/install.md](docs/reference/install.md)).

Install with the one-liner (no sudo; it installs under `~/.local/share/capstan` and links `~/.local/bin/cstan`):

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
```

Pin a version (0.4.0 or newer) or uninstall:

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --version 0.4.0
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --uninstall
```

### Dashboard (cstan-dash)

`cstan dash` runs `cstan-dash`, the terminal dashboard: about 1% of a core and 7 MB idle (the measurements, with the method and the other cases, are in [docs/research/rust-dash-performance.md](docs/research/rust-dash-performance.md)).

**With the installer.** The one-liner above installs `cstan-dash` next to `cstan` (Linux x64 and arm64). It downloads `cstan-dash-<version>-<target>`, checks it against `SHA256SUMS` (a mismatch aborts the install) and puts it at `~/.local/share/capstan/current/bin/cstan-dash`.

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --no-dash                        # skip it
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --dash-binary ./cstan-dash       # install one you already have
```

`--dash-binary` checks the file against a `SHA256SUMS` beside it when there is one, and warns that it is unverified when there is not. The full option list is in the [Install reference](docs/reference/install.md).

**By hand.** Download `cstan-dash-<version>-linux-x64` or `cstan-dash-<version>-linux-arm64` and `SHA256SUMS` from the [Releases page](https://github.com/akhilrs/capstan/releases), check it, and put it somewhere `cstan dash` looks:

```sh
sha256sum --check --ignore-missing SHA256SUMS
chmod +x cstan-dash-<version>-linux-x64
mkdir -p ~/.local/share/capstan/current/bin
mv cstan-dash-<version>-linux-x64 ~/.local/share/capstan/current/bin/cstan-dash
```

Any directory on your `PATH`, or a path named by `CSTAN_DASH_BIN`, works too.

**From source.** Install Rust with [rustup](https://rustup.rs) (`dash/rust-toolchain.toml` selects the stable toolchain with clippy and rustfmt), then build:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --locked --manifest-path dash/Cargo.toml    # writes dash/target/release/cstan-dash
```

**Which dashboard `cstan dash` runs.** It takes the first executable file that answers `--version` from this list:

1. `CSTAN_DASH_BIN`, an absolute path.
2. Beside the real path of the running `cstan` (the installer's `current/bin/`).
3. `${XDG_DATA_HOME:-$HOME/.local/share}/capstan/current/bin/cstan-dash`.
4. The first `cstan-dash` on `PATH`.

With none found, `cstan dash` prints `cstan: cstan-dash was not found; install it beside cstan, put it on PATH or set CSTAN_DASH_BIN to its absolute path (docs/reference/install.md)` and exits 2. `cstan-dash --version` prints `cstan-dash <version>`.

> [!NOTE]
> `cstan-dash` binaries come from tagged releases, on the [Releases page](https://github.com/akhilrs/capstan/releases). CI runs on branches and pull requests do not publish them.

Re-running the installer upgrades. Options, layout, checksum verification and troubleshooting: [Install reference](docs/reference/install.md).

> [!NOTE]
> The installer downloads a GitHub release. The binaries and `SHA256SUMS` are on the [Releases page](https://github.com/akhilrs/capstan/releases). To build from source, see [Development](#development).

Then, in the root of the git repository you want the team to work on. If the folder is not a repository yet, or has no commit, run `cstan init --git` instead: it runs `git init` when needed, lists the files it will commit, and creates an initial commit (`chore: initial commit`) of the current files, honouring your `.gitignore`. Capstan never runs git init or commits without that flag; `cstan start` refuses to run until the requirement is met.

```sh
cstan init            # (or: cstan init --git) creates .capstan/, a starter capstan.toml and roles/designer.md
cstan config check    # validates capstan.toml and prints the resolved config
cstan start           # starts the controller and launches the PM in Herdr
cstan dash            # optional: watch the team
```

Switch to the PM's Herdr pane and tell it what you want built. `cstan stop` shuts the controller down.

> [!NOTE]
> `cstan init` adds `/.capstan/` to the repository's local git exclude. The folder holds the ledger and the operator key (`0600`); keep it out of commits.

## Configuration

`cstan init` writes a commented starter `capstan.toml` and, next to it, `roles/designer.md`, the designer role's prompt file. Keys are checked strictly: an unknown key is an error. A trimmed example:

```toml
schema_version = 1

[limits]
max_workers = 3                 # workers active at once (PM excluded), at most 16

[layout]
spawn = "pane"                  # or "tab"
pm_width_percent = 60

[supervision]
enabled = true
check_seconds = 300

[worktree]
setup = "./scripts/setup.sh"    # runs once in each new worktree
# teardown = "..."              # runs before a worktree is removed

[defaults.Developer]            # per role kind; a role's own value wins
model = "claude-sonnet-5-5"
permission_mode = "acceptEdits"

[env]
pass = ["NEXORA_API_KEY"]       # extra variables copied into every agent

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You implement code changes. Work only in your own worktree ..."

# Optional tables, off until enabled: [architect], [operator], [prompt_relay],
# [researcher] with [mcp_servers.<name>], and [nexora].
```

<details>
<summary><b>All tables at a glance</b></summary>

| Table                                  | What it sets                                                                                                         |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| top level                              | `schema_version`, `herdr_session`                                                                                    |
| `[project]`                            | `name` (must match the initialized project)                                                                          |
| `[limits]`                             | `max_workers`                                                                                                        |
| `[layout]`                             | `spawn`, `split`, `pm_width_percent`, `min_pane_columns`, `min_pane_rows`                                            |
| `[worktree]`                           | `setup`, `setup_timeout_seconds`, `teardown`, `teardown_timeout_seconds`                                             |
| `[supervision]`                        | `enabled`, `check_seconds`                                                                                           |
| `[defaults]`, `[defaults.<Kind>]`      | `model`, `permission_mode` (`default`, `acceptEdits`, `plan`, `auto`)                                                |
| `[env]`                                | `pass`                                                                                                               |
| `[hosts.<name>]`                       | `kind` (`claude`, `codex`, `omp`), `command`, timeouts                                                               |
| `[roles.<name>]`                       | `kind`, `host`, `model`, `permission_mode`, `allow`, `deny`, `hooks`, `mcp`, `prompt` or `prompt_file`               |
| `[architect]`                          | `enabled`, `role`, `plan_review`, `reviewer_role`, `max_packages`, `count_toward_worker_limit`, `high_risk_triggers` |
| `[operator]`                           | `enabled`, `role`, `auto_approve`, `auto_approve_prefix`, timeouts, grant and full-auto limits                       |
| `[prompt_relay]`                       | `enabled`, `capture_ttl_seconds`                                                                                     |
| `[researcher]`, `[mcp_servers.<name>]` | `enabled`, `role`, `output_dir`, `user_agent`; `command`, `args`                                                     |
| `[nexora]`                             | `track`, `default_action`                                                                                            |
| `[notifications]`, `[timers]`          | notification channels; delivery, stall and wake timers                                                               |

The full reference, with defaults, ranges, Codex and OMP workers and the teardown example, is in [Configuration reference](docs/reference/configuration.md). The schema itself is [`src/config/capstan-config.ts`](src/config/capstan-config.ts).

</details>

## Roles

A role has a free name and one of four kinds; the kind decides what the agent may do. The starter config defines the first six rows; the last three are optional and come commented out.

| Role         | Kind       | Does                                                                                                       |
| ------------ | ---------- | ---------------------------------------------------------------------------------------------------------- |
| `pm`         | PM         | Talks to you, plans, spawns and releases workers, requests reviews, integrates. Never edits project files. |
| `developer`  | Developer  | Implements changes in its own worktree and branch, then reports a commit.                                  |
| `designer`   | Developer  | Designs and builds UI: Claude Design, a brief, directions, build, Playwright checks (`roles/designer.md`). |
| `reviewer`   | Verifier   | Reviews one commit or integration, answers pass or findings once, is ended. Cannot write files.            |
| `tester`     | Verifier   | Runs the real checks and reports what passed and failed.                                                   |
| `supervisor` | Supervisor | Watches active workers and raises findings. Read and report only.                                          |
| `architect`  | Developer  | Writes plans, runs reviews and integration for plan work, signs off. Never edits or merges.                |
| `operator`   | Developer  | Proposes shell commands and restarts through `cstan op`; has no shell of its own.                          |
| `researcher` | Developer  | Researches on the web and commits one Markdown report under `output_dir`.                                  |

Codex and OMP hosts can run Developer and Verifier roles; PM, Supervisor, Architect, Operator and Researcher stay on Claude Code. See [Running a worker on Codex or OMP](docs/reference/configuration.md#running-a-worker-on-codex-or-omp).

## Command reference

Run `cstan` with no arguments for the usage line. Every routed command accepts `--json`. Long text belongs in a quoted heredoc: `cstan send developer-1 "$(cat <<'EOF' ... EOF)"`.

<details>
<summary><b>Project and daemon</b> (you, from the project directory)</summary>

| Command                                          | Purpose                                                       |
| ------------------------------------------------ | ------------------------------------------------------------- |
| `cstan init`                                     | Create `.capstan/` and a starter `capstan.toml`.              |
| `cstan start` / `cstan stop`                     | Start the controller and launch the PM / stop the controller. |
| `cstan ping`                                     | Check that the controller answers.                            |
| `cstan config check` / `config sync`             | Validate the config / write role definitions into the ledger. |
| `cstan herdr-config`                             | Print an optional Herdr `config.toml` snippet.                |
| `cstan pm restart`                               | Replace the PM session, seeded from the ledger.               |
| `cstan resolve <message-id> retry\|skip\|cancel` | Settle a blocked message.                                     |
| `cstan cancel <message-id>`                      | Cancel a message.                                             |

</details>

<details>
<summary><b>Look and read</b></summary>

| Command                             | Purpose                                                                               |
| ----------------------------------- | ------------------------------------------------------------------------------------- |
| `cstan status [--json]` / `--watch` | Project state.                                                                        |
| `cstan inspect <id>`                | One ledger record.                                                                    |
| `cstan dash`                        | Read-only terminal dashboard.                                                         |
| `cstan inbox [<agent-id>]`          | Own pending messages (a worker pulls all its mail), or an agent's mailbox (operator). |
| `cstan observe <agent-id> [lines]`  | Another agent's recent screen (PM and Supervisor).                                    |

</details>

<details>
<summary><b>Messaging and workers</b></summary>

| Command                                                              | Purpose                                                   |
| -------------------------------------------------------------------- | --------------------------------------------------------- |
| `cstan send <agent-id\|@pm> "<text>"`                                | Queue a message.                                          |
| `cstan wait`                                                         | Block for new messages (PM and workers).                  |
| `cstan ack <message-id>`                                             | Acknowledge a message.                                    |
| `cstan spawn <role> [--task <ref>] [--type <type>] [--title <text>]` | Start a worker; `--task` names its branch after the task. |
| `cstan release <agent-id>`                                           | End a worker, free its pane and worktree.                 |
| `cstan replace <agent-id>`                                           | Replace a lost or stuck worker.                           |
| `cstan pause [<agent-id>] --reason "<text>" [--interrupt]`           | Hold one agent or the whole run (you or the PM).          |
| `cstan resume [<agent-id>] --reason "<text>"`                        | Release a pause.                                          |

</details>

<details>
<summary><b>Delivery: report, review, integrate</b></summary>

| Command                                                  | Purpose                                             |
| -------------------------------------------------------- | --------------------------------------------------- |
| `cstan report <commit> "<summary>"`                      | Report a finished commit (full 40-character id).    |
| `cstan request-review <report-or-integration-id> [role]` | Start an independent review.                        |
| `cstan review pass\|findings "<text>"`                   | Reviewer: answer once.                              |
| `cstan integrate <report-id>...`                         | Squash reviewed reports onto an integration branch. |
| `cstan integrate confirm\|discard <integration-id>`      | Settle a merged integration.                        |

</details>

<details>
<summary><b>Plans, supervision, Nexora, prompt relay and Operator</b></summary>

| Command                                                                         | Purpose                                                                                                                                    |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `cstan plan open\|submit\|show\|assign\|signoff\|cancel ...`                    | Planned work with the Architect. `assign` waits for reviewed dependencies unless `--early "<reason>"`; `integrate` keeps dependency order. |
| `cstan finding <agent-id> <severity> "<evidence>" "<correction>" "<done-when>"` | Supervisor: raise a finding.                                                                                                               |
| `cstan finding check <finding-id> resolved\|unresolved "<evidence>"`            | Supervisor: check a finding.                                                                                                               |
| `cstan link requirement\|plan\|package <ref-id> <nexora-id> [<state>]`          | Record a Nexora link.                                                                                                                      |
| `cstan link bind <requirement-ref-id> <agent-id>`                               | Tie a requirement to its developer.                                                                                                        |
| `cstan prompt show <agent-id>` / `prompt answer <relay-id> --hash <h> ...`      | Relay a worker's permission prompt, or an unrecognised blocking dialog as Esc only.                                                        |
| `cstan op propose\|decide\|show\|cancel\|grants\|revoke\|full-auto ...`         | Operator proposals, grants and full auto.                                                                                                  |

</details>

Who may run each command, the controller's message prefixes and the exit codes are in the [command reference](docs/reference/commands.md).

## Safety model

> [!IMPORTANT]
> Capstan runs on one machine you trust, as your user, with full network access. A worktree isolates changes; **it is not a security boundary**. The controls below are guards that stop mistakes and obvious misuse, not a sandbox. See [`decisions/DEC-005-capstan-v2-direction.md`](decisions/DEC-005-capstan-v2-direction.md) for the accepted risks.

| Guard                        | What it protects                                                                                                                                                                                                                                                                                                                                     |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **You merge**                | Capstan never pushes and never merges into your branch. `integrate confirm` is refused until the integration is in HEAD.                                                                                                                                                                                                                             |
| **Controller-checked facts** | Reports, reviews and integrations are recorded only when an agent runs a `cstan` command and the controller checks what it can. Agents authenticate with a per-agent token; you with `.capstan/operator.key`.                                                                                                                                        |
| **Role deny lists**          | Claude Code tool rules per role: developers deny `git push`, reviewers and the Supervisor deny file writes. They are tool rules, not enforcement at the OS level.                                                                                                                                                                                    |
| **Operator**                 | The Operator only proposes. A command runs once, after a PM approval bound to a hash of its exact text, unless it is on a short read-only allowlist or covered by a session grant. A denylist (`push`, `rm`, `sudo`, `curl`, ...) always forces approval, except in full auto, which you switch on for a limited time and which removes every guard. |
| **Prompt relay**             | Nobody answers another agent's permission prompt on their own. The PM types only the option you chose, against a hash of the prompt it showed you; a dialog it does not recognise gets Esc only, and only on a conservative screen match.                                                                                                            |
| **Researcher**               | `permission_mode = "default"`, a strict allow list and curl deny lists: GET only, no uploads, no output files, writes only under `output_dir`. Guards, not a sandbox; it reads untrusted pages.                                                                                                                                                      |
| **Codex and OMP workers**    | Run unsandboxed (`danger-full-access`, `yolo`); `cstan config check` warns about every such role.                                                                                                                                                                                                                                                    |

Details: [Operator reference](docs/reference/operator.md), [Researcher reference](docs/reference/researcher.md).

## Nexora tracking

With `[nexora] track = "ask"` or `"always"`, the PM mirrors each requirement into Nexora as an epic and each work package as a story, and records the ids and states it wrote with `cstan link`. The controller never calls Nexora; it only keeps the links and shows a `Nexora drift` section in `cstan status` when what was written differs from what the ledger now wants. Connection details stay in `.nexora.toml`, which Capstan never reads. `track = "never"` removes every Nexora instruction from the PM.

## Project layout

```text
rust/                    the cargo workspace (cstan and the daemon)
  crates/cstan/            the cstan front end: every command
  crates/daemon/           the controller daemon, its loops and wiring
  crates/kernel/           ledger rules: messaging, auth, plans, reports, integrations
  crates/ledger/           SQLite storage, migrations and the project lock
  crates/config/           capstan.toml schema, loader and starter file
  crates/launcher/         spawn, release, replace, worktrees, prompts
  crates/herdr/            Herdr adapter, host parsers, prompt relay
  crates/operator/         Operator policy, runs and the controller restart
  crates/client/ wire/     the socket client and the wire protocol
  crates/blackbox/         black-box suites that run the built cstan
dash/                    cstan-dash, the terminal dashboard (its own cargo tree)
tools/release/           cstan-release, used by scripts/release.sh
migrations/              SQLite schema migrations
scripts/                 check.sh (the gate), release.sh, install and smoke tests
docs/                    reference, design notes, test map and assets
decisions/               design decisions (DEC-001 to DEC-006)
```

## Development

Run from source instead of installing a release. You need git and [rustup](https://rustup.rs) (`rust/rust-toolchain.toml` selects the toolchain):

```sh
git clone <this repository> capstan && cd capstan
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --locked --manifest-path rust/Cargo.toml -p cstan-front   # rust/target/release/cstan
```

Day to day:

```sh
scripts/check.sh                       # the single gate: every stage except the release build
scripts/check.sh rust                  # one stage: rust | dash | tools | scripts | release | test-map (CI runs one job per stage)
CSTAN_RELEASE_BUILD=1 scripts/check.sh # also builds the release binaries, so smoke-binary.sh runs on them
cargo test --locked --manifest-path rust/Cargo.toml -p capstan-blackbox   # the black-box suites, about a minute
CSTAN_LIVE=1 scripts/check.sh          # adds the live suites (a real Herdr and Claude Code)
```

`CARGO_TARGET_DIR` puts the build outside the checkout and `CSTAN_CHECK_JOBS` limits cargo's jobs. The release binaries are static musl builds of `cstan` and `cstan-dash` for Linux x64 and arm64, built by the Release workflow (`.github/workflows/release.yml`) with cargo-zigbuild; see [docs/binary.md](docs/binary.md). The daemon is described in [docs/daemon.md](docs/daemon.md), and the test map in [docs/test-map/](docs/test-map/core.md).

Tests never touch a real Herdr session: with `CAPSTAN_LAUNCH=off`, or without a `capstan.toml`, the controller launches no agents.

## Upgrading from 0.3

Capstan 0.4 is Rust only: `cstan` is one static binary that is also the controller daemon, and `cstan-dash` is the dashboard. Releases up to 0.3.0 were a Node.js program (an npm package, then a Node standalone binary). To move an existing Node-based install (0.1.1 to 0.3.0) to 0.4.0, once release 0.4.0 is published (the installer installs nothing before that):

1. In each project, run `cstan stop` and wait until `cstan ping` fails. Running controllers keep the old build until they are restarted.
2. Remove a leftover global npm install, if any: check with `npm ls -g capstan-controller` and `npm unlink -g capstan-controller` (or `npm uninstall -g`). The installer warns when an earlier `cstan` on your `PATH` shadows the new one.
3. Run the one-liner installer from [Quick start](#quick-start). It replaces the old `current` (Node standalone binary, `cstan-node` or the npm prefix layout) in place. For a fresh start, run it with `--uninstall` first, then install.
4. Check `cstan --version` and `cstan dash`.
5. Delete the `[daemon]` table from `capstan.toml` (and unset `CSTAN_DAEMON`), then run `cstan start` in each project. The ledger migrates on start; a project's `.capstan/` is never touched.

Details are in the [Install reference](docs/reference/install.md#upgrading-from-03). What changes when you upgrade:

- Install with `install.sh`; Node.js and npm are no longer needed. The installer replaces an earlier install and warns about a leftover `npm link` that shadows `cstan` (see the [Install reference](docs/reference/install.md)).
- Stop the controller of each project with the old `cstan stop` before upgrading, then run `cstan start` with the new one. The ledger is the same and migrates in place.
- Delete the `[daemon]` table from `capstan.toml` and unset `CSTAN_DAEMON`. `implementation = "rust"` still loads, with a warning; `implementation = "node"` and `CSTAN_DAEMON=node` are errors. See [docs/daemon.md](docs/daemon.md).
- `CSTAN_DAEMON_BIN`, `CSTAN_NODE_CLI` and `CSTAN_NODE` are no longer read. `CSTAN_DASH=node` is gone: there is no Node dashboard.
- A `[worktree] setup` that ran `npm ci` is yours to keep; Capstan itself no longer needs npm.
- The repository still holds the old TypeScript sources and their tests (`src/`, `test/`, `package.json`) until they are deleted; they are frozen and are not part of a release. A source checkout is built and checked with cargo as described above.

## Contributing

Issues and pull requests are welcome; contributions are accepted under the [MIT License](LICENSE). Before you open one:

- run `scripts/check.sh` and keep it green;
- write commit subjects as [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) (`<type>[(scope)][!]: <description>`; types `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `style`, `revert`);
- name branches `<type>/<task-id>-<slug>` (or `chore/<agent-id>-<slug>` for ad-hoc work); Capstan creates worker and integration (`integration/<plan-id>-<slug>`) branches for you, and older `capstan/<agent>-g<n>` branches still work;
- leave AI attribution out of commits and pull requests: no `Co-Authored-By` line naming Claude, no `Claude-Session` line, no "Generated with Claude Code" footer (`cstan report` refuses commits that have one);
- releases follow [SemVer](https://semver.org): `scripts/release.sh` picks the version from the commits, writes `VERSION`, the version files and `CHANGELOG.md`, and tags locally (`--dry-run` previews it, `--version X.Y.Z` overrides it); pushing the tag starts the Release workflow. See [Branches, commits and releases](docs/reference/workflow.md#branches-commits-and-releases) and [Releases](docs/reference/commands.md#releases);
- describe behaviour as the code has it, and update the [reference docs](docs/reference/) with any change to commands, config keys or messages;
- for design changes, read the relevant note in [`docs/design/`](docs/design/) and [`decisions/`](decisions/) first.

## License

Capstan is released under the [MIT License](LICENSE).

## Further reading

- [How Capstan works](docs/reference/workflow.md): concepts, delivery flow, messaging, worker lifecycle, findings, plans, prompt relay
- [Install reference](docs/reference/install.md) · [Configuration reference](docs/reference/configuration.md) · [Command reference](docs/reference/commands.md) · [Operator](docs/reference/operator.md) · [Researcher](docs/reference/researcher.md)
- [`docs/design/architect-role.md`](docs/design/architect-role.md), [`docs/design/researcher-role.md`](docs/design/researcher-role.md), [`docs/design/cstan-dash-v2.md`](docs/design/cstan-dash-v2.md)
- [`decisions/DEC-005-capstan-v2-direction.md`](decisions/DEC-005-capstan-v2-direction.md): the current direction and trust model
