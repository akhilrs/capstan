# Capstan

Capstan is a controller for a small team of AI coding agents. You talk to one agent, the **PM**. The PM spawns **worker agents** (developers, designers, reviewers, testers), hands them tasks and reports back to you. Every agent is an interactive session (Claude Code first; `codex` and `omp` hosts are accepted by the configuration loader) that runs in a Herdr pane inside its own git worktree. A separate **controller daemon** carries all messages between agents and records every workflow fact in a SQLite ledger. You merge the result yourself.

Everything is driven through one command, `cstan`.

> Trust model: Capstan runs on one VM that you trust, as your user, with full network access. A worktree isolates changes; it is not a security boundary. See `decisions/DEC-005-capstan-v2-direction.md` for the accepted risks.

## Contents

- [Concepts](#concepts)
- [Install and build](#install-and-build)
- [Start a project](#start-a-project)
- [Configuration: `capstan.toml`](#configuration-capstantoml)
- [The delivery flow](#the-delivery-flow)
- [Messaging](#messaging)
- [Worker lifecycle](#worker-lifecycle)
- [Supervisor findings](#supervisor-findings)
- [Status, observe and the dashboard](#status-observe-and-the-dashboard)
- [`cstan` command reference](#cstan-command-reference)
- [Controller messages](#controller-messages)
- [Exit codes](#exit-codes)
- [Development](#development)
- [Further reading](#further-reading)

## Concepts

### Roles and seats

A role has a free name (for example `developer`, `designer`, `reviewer`, `tester`) and one of four **kinds**. The kind decides what the agent may do:

| Kind | Purpose | What it may run |
| --- | --- | --- |
| `PM` | Talks to you, plans, spawns and releases workers, sends tasks, reads reports, requests reviews, integrates, resolves delivery problems. Never edits project files. | `spawn`, `release`, `replace`, `send`, `inbox`, `wait`, `ack`, `request-review`, `integrate`, `observe`, `status` |
| `Developer` | Implements or designs changes in its own worktree and branch (`developer` and `designer` in the starter config). | `ack`, `inbox`, `send @pm`, `report`, `status` |
| `Verifier` | Reviews one commit (`reviewer`) or runs tests (`tester`). A reviewer answers a review request once and is then ended by the controller. | `ack`, `inbox`, `send @pm`, `report`, `review`, `status` |
| `Supervisor` | Watches the other agents and raises findings. Read and report only. | `status`, `observe`, `finding`, `inbox`, `ack` |

`resolve` and `cancel` are registered as operator-only in `ROUTES` (`src/daemon.ts`), although the PM prompt (`src/prompts.ts`) tells the PM to run the `cstan resolve` a delivery notice names. Run them yourself from the project directory; an agent token is refused with `forbidden`.

A **seat** is one running agent of a role. Agents get ids such as `developer-1`; a replacement gets a new id and a higher generation (`g1`, `g2`, ...). The PM seat is created at `cstan start`; worker seats are created by `cstan spawn`. At most `limits.max_workers` workers (every agent except the PM) may be active at once.

### Controller and ledger

The **controller** is a long-running daemon (`cstan daemon`, started for you by `cstan start` and by any operator command). It holds a single-instance lock on the project, listens on a Unix socket in `.capstan/state/` and owns:

- **The ledger**, a SQLite database (`.capstan/state/controller.sqlite`, schema in `migrations/`) with agents, messages, reports, reviews, integrations, findings and the history of each.
- **All agent-to-agent messaging.** Only the controller types into worker panes, starts and closes agents in Herdr. The PM's pane is never typed into except for a one-line `Run cstan inbox` wake-up when it is idle.
- **Verification of reports.** Completion is never inferred from terminal text: it is recorded when an agent runs a `cstan` command, and the controller checks what it can (for a report: that the commit exists on that worker's branch after the branch started).

Agents authenticate to the daemon with a per-agent token (`CAPSTAN_TOKEN`) and socket path (`CAPSTAN_SOCKET`) in their environment. The operator (you) authenticates with the credential file `.capstan/operator.key`, which only works from the project directory.

## Install and build

Requirements: Node.js 24 (`engines`: `>=24 <25`), git, and a Herdr installation for launching agents. Claude Code is the host used by the starter configuration.

```sh
npm install
npm run build        # compiles to dist/ and copies migrations
npm link             # or put dist/src/cli.js on your PATH as `cstan`
```

`package.json` declares the `cstan` binary as `dist/src/cli.js`. Without `capstan.toml`, or with `CAPSTAN_LAUNCH=off` in the environment, the daemon runs without Herdr and launches no agents (the test suite uses this).

## Start a project

Run these in the root of your project's git repository.

```sh
cstan init            # creates .capstan/ and a starter capstan.toml
cstan config check    # validate capstan.toml; prints the resolved config
cstan start           # start the controller and launch the PM in Herdr
cstan dash            # watch the team (optional)
```

- `cstan init` creates `.capstan/` (mode `0700`) with `project.json`, the operator key `operator.key` (`0600`) and the `state/` directory, and writes a starter `capstan.toml` unless one exists. In a git repository it adds `/.capstan/` to the repository's local `info/exclude`. The project name is the directory name. It refuses to run if `.capstan/` already exists.
- `cstan herdr-config` prints an optional snippet for Herdr's `config.toml` (sidebar rows that show the project). Capstan never edits Herdr's config.
- `cstan start` starts the daemon (if it is not running) and, when `capstan.toml` exists, launches the PM. It prints `running`, the daemon `pid`, whether it was `started` now, and a `launch` result; it exits `4` if the launch failed. A daemon started before `capstan.toml` existed reads it only at start: run `cstan stop` and `cstan start`.
- `cstan stop` shuts the daemon down.
- After starting, switch to the PM's Herdr pane and tell the PM what you want built.
- `cstan config sync` writes the role definitions from `capstan.toml` into the ledger while the daemon is stopped (the running daemon also syncs them at start).

## Configuration: `capstan.toml`

`cstan init` writes this file. Keys are checked strictly: an unknown key is an error (`cstan config check` shows errors and warnings). The authoritative schema is `src/config/capstan-config.ts`.

| Table | Keys | Notes |
| --- | --- | --- |
| top level | `schema_version` (1), `herdr_session` | Herdr session name; default `default`. |
| `[project]` | `name` | Must match the initialized project name. |
| `[limits]` | `max_workers` | Default 3, at most 16. |
| `[layout]` | `spawn` (`tab` or `pane`), `split` (`auto`, `right`, `down`), `pm_width_percent`, `min_pane_columns`, `min_pane_rows` | Starter file sets `spawn = "pane"`, `pm_width_percent = 60`. |
| `[worktree]` | `setup`, `setup_timeout_seconds` | Optional. `setup` is a shell command run once per new worktree with `sh -c`, the worktree as the working directory, e.g. `setup = "npm install"`. It applies to workers, replacements and the architect, not the PM. `setup_timeout_seconds` is 1 to 3600, default 600, and is refused without `setup`. A failing or timed-out setup aborts the spawn. The setup command runs with the same filtered environment as agents (the default allowlist plus `[env] pass`), not the daemon's full environment. `setup` is trusted project config, like the other commands in `capstan.toml`: anyone who can edit the file can run commands. |
| `[supervision]` | `enabled`, `check_seconds` | While workers are active the controller keeps one Supervisor running and sends it a routine check (default every 300 s). A Supervisor uses model usage; set `enabled = false` to turn it off. |
| `[defaults]`, `[defaults.PM]`, `.Supervisor`, `.Developer`, `.Verifier` | `model`, `permission_mode` | Used when a role sets none. `permission_mode` is one of `default`, `acceptEdits`, `plan`, `auto`. |
| `[env]` | `pass` | Extra environment variable names copied from where the daemon started into every agent. |
| `[hosts.<name>]` | `kind` (`claude`, `codex`, `omp`), `command`, `shell_command_timeout_seconds`, `wait_timeout_seconds` | `codex` and `omp` hosts run unattended with full access; `cstan config check` warns about it. |
| `[roles.<name>]` | `kind` (`PM`, `Developer`, `Verifier`, `Supervisor`), `host`, `model`, `permission_mode`, `allow`, `deny`, `hooks` (`off` or `inherit`), `prompt` or `prompt_file` | Role names match `^[a-z][a-z0-9-]{0,31}$`. A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, unless the role sets `deny` itself. |
| `[notifications]` | `herdr`, `fallback` | At least one must be true. |
| `[timers]` | `max_deferral_seconds`, `max_busy_deferral_seconds`, `pm_ack_timeout_seconds`, `pm_notify_after_seconds`, `notify_interval_seconds`, `stall_after_seconds`, `worker_ack_timeout_seconds`, `finding_check_seconds`, `pm_wake_after_seconds`, `pm_wake_interval_seconds` | Defaults and ranges are in `TIMER_DEFAULTS`. |

The starter file defines these roles: `pm` (PM), `developer` and `designer` (Developer), `reviewer` and `tester` (Verifier), and `supervisor` (Supervisor), all on a `claude` host. Developer roles deny `git push`; the reviewer and supervisor also deny file-writing tools. Each agent's system prompt is a built-in command reference for its kind (`src/prompts.ts`) followed by the role's own `prompt`.

Per-project controller files live in `.capstan/`: `project.json` (limits `maxSlices`, `maxRunMs`, `maxDispatches` shown by `cstan status`), `operator.key`, `daemon.log`, and `state/` (`controller.sqlite`, `control.sock`, `notifications.jsonl`).

## The delivery flow

The PM runs these steps on your behalf. You do the last one.

```text
 you ──task──▶ PM
                │ 1. cstan spawn <role>            controller: new pane + worktree + branch
                │ 2. cstan send <agent> "<task>"   controller queues, types it when worker idle
                ▼
             worker ── works, commits on its own branch (never pushes or merges)
                │ 3. cstan report <sha> "<summary>"
                ▼
        controller verifies commit ──▶ message "Verified report ..." to PM
                │
                │ 4. cstan request-review <report-id> [role]
                ▼
        controller spawns reviewer at that commit ── cstan review pass|findings "<text>"
                │                                          (reviewer ended by controller)
                ▼
        message "Review ..." to PM
          ├─ findings ─▶ PM sends them to the developer ─▶ new verified report ─▶ new review
          └─ pass
                │ 5. cstan integrate <report-id>...
                ▼
        controller merges the reports onto  capstan/integration/<id>  (no checkout)
          ├─ conflicted ─▶ nothing left behind; PM assigns a developer to resolve, then report/review again
          └─ merged
                │ 6. cstan request-review <integration-id> [role]
                ▼
        review of the integration  ── pass
                │ 7. YOU merge the integration branch into the project's HEAD
                │ 8. cstan integrate confirm <integration-id>   (or: integrate discard <id>)
                ▼
        controller removes the integration branch;  PM: cstan release <agent-id>
```

Step by step, as the code behaves (`src/commands.ts`, `src/controller/core.ts`, `src/launcher.ts`, `src/reviews.ts`, `src/integration.ts`):

1. **Spawn.** `cstan spawn <role>` creates an agent of that role in a new Herdr pane and git worktree on a branch named `capstan/<agent-id>-g<generation>`, cut from the project's HEAD. It is refused with `worker_limit` when `limits.max_workers` workers are active.
2. **Send the task.** `cstan send <agent-id> "<text>"` queues a message. See [Messaging](#messaging). The task should state the acceptance criteria and that the worker commits on its own branch and never pushes or merges.
3. **Report.** When finished, the worker runs `cstan report <40-char-commit> "<one-line summary>"`. The controller checks that the commit exists on the worker's branch, is not older than the branch's start, and is on that branch; otherwise the report is recorded as `rejected` and the command fails. An accepted report is announced to the PM as a message from `controller` starting with `Verified report`. This is a fact the controller checked, not a review. A repeat report of the same commit writes nothing new. Reports are rate-limited to 10 a minute per agent.
4. **Request review.** Only the PM runs `cstan request-review <report-id> [role]`. The report must be accepted. The controller spawns a fresh Verifier (the named role, or the configured one) at that commit and sends it a `Review request`. The reviewer answers exactly once with `cstan review pass "<text>"` or `cstan review findings "<text>"`, after which the controller ends it. The PM gets the verdict as a `Review` message from `controller`. On findings the PM sends them to the developer, which fixes, reports again and gets a new review round.
5. **Integrate.** `cstan integrate <report-id>...` (PM or operator) merges reports that each have a passing latest review, in the order given, onto a new branch `capstan/integration/<integration-id>` cut from the project's HEAD, without checking anything out. The answer is `merged` (branch and head commit), `conflicted` (the report and files) or `failed`. The controller never resolves a conflict and leaves nothing behind: the PM assigns a developer to resolve it as a new candidate, then report, review and integrate again.
6. **Review the integration.** A merged result needs its own review: `cstan request-review <integration-id> [role]`.
7. **You merge.** After a passing review, merge the integration branch into the project's HEAD yourself. Capstan never pushes or merges your branch.
8. **Confirm or discard.** `cstan integrate confirm <integration-id>` is accepted only when the integration is `merged`, no review of it is open, its latest review passed, and its branch is already in the project's HEAD (otherwise `not_in_head`); the controller then removes the branch. `cstan integrate discard <integration-id>` drops a merged integration you will not use.
9. **Release.** `cstan release <agent-id>` ends a worker and frees its pane and worktree. The branch is kept when it holds commits.

States recorded in the ledger: reports `accepted` or `rejected`; reviews `started`, `passed`, `findings`, `failed`, `cancelled`; integrations `running`, `merged`, `conflicted`, `failed`, `confirmed`, `discarded`.

## Messaging

All agent-to-agent text goes through the controller, one strict FIFO per recipient.

- **Send.** `cstan send <agent-id|@pm> "<text>"`. Plain text only (a message may not start with `/`, `!`, `#`, `?` or `@`), size-limited, and may not contain a line that imitates a Capstan message frame. A worker (Developer or Verifier) may send only to the PM. Sending to yourself is refused.
- **Delivery to a worker.** The controller types the message into the worker's pane as `[capstan message <message-id> from <sender>]` followed by the text. It waits while the worker is busy, blocked at a prompt, or has text in its input line (deferral reasons `agent_busy`, `agent_blocked`, `input_not_empty`). After `timers.max_deferral_seconds` it reads and logs the typed text, clears the input line and sends. A message that waits longer than `timers.max_busy_deferral_seconds` expires and the PM is told.
- **Delivery to the PM.** Nothing is typed into the PM's pane. The PM reads with `cstan inbox` (the next message plus any read but unacknowledged) or `cstan wait` (blocks up to the host's wait timeout, default 90 s, and prints unacknowledged messages). When the PM is idle and unread mail has waited `timers.pm_wake_after_seconds`, the controller types `Run cstan inbox`.
- **Acknowledge.** `cstan ack <message-id>`. Reading does not acknowledge; the next message to that agent is delivered only after the current one is acknowledged.
- **Message states:** `queued`, `deferred`, `sent`, `acked`, `acked_late`, `unacked`, `expired`, `cancelled`, `failed`.
- **Delivery problems.** When a message to a worker becomes `unacked`, `expired` or `failed`, that worker's queue is blocked behind it and the PM gets a `Delivery problem` notice naming the command to run:
  `cstan resolve <message-id> retry|skip|cancel ["<note>"]` — `retry` types it once more (the recipient may already have received it), `skip` counts it handled, `cancel` drops it. `cstan cancel <message-id>` is the same as `resolve ... cancel`.
- Operators can read an agent's mailbox with `cstan inbox <agent-id>`.

## Worker lifecycle

- **Spawn:** see step 1 above. `cstan spawn` and `cstan release` can take minutes; run them with a long timeout, and after any timeout run `cstan status` before retrying (the worker may already exist and counts against the limit).
- **Release:** `cstan release <agent-id>` removes the pane and the worktree. The answer reports `paneClosed` and `worktreeRemoved`; `false` means that cleanup is still pending and the next spawn or release finishes it (`cstan status` shows `cleanupFailed` and `orphanPanes`). `agent_not_active` means the worker is already gone. Reviewers are released by the controller after they answer.
- **Replace:** `cstan replace <agent-id>` replaces a lost or stuck worker. The controller releases it (when still running) and starts a new agent of the same role with a new id, a new branch that starts at the predecessor's last accepted report (else HEAD), and a seed built from the ledger (its instructions and their states, accepted reports, open findings). Nothing is re-sent; the PM sends what still matters. It is refused while an integration is running and for an agent that was already replaced.
- **Lost:** when Herdr no longer finds an agent's pane or process (or its pane was already gone when the daemon started), the controller ends the agent in the ledger and sends the PM an `Agent ... is lost` message with its branch and its unacknowledged messages. The controller never replaces it by itself: use `cstan replace` or `cstan release`.
- **Stalled / blocked:** the controller watches worker activity. `Agent stalled` means no activity while working for `timers.stall_after_seconds`; `Agent blocked` means it waits at a dialog or permission prompt only a person can answer. Use `cstan observe <agent-id>`, then replace the worker or tell the user. Nobody answers another agent's permission prompt.
- **PM restart:** `cstan pm restart` (operator) replaces the PM session with a new one that starts from a summary of the ledger (open work, unacknowledged messages).

## Supervisor findings

While workers are active the controller keeps one Supervisor running (`[supervision]`) and sends it a `Routine check` message every `check_seconds`. The Supervisor looks at agents with `cstan status` and `cstan observe`, and only raises a finding when an agent is clearly stuck (the same failure repeated, a retry that cannot work):

- `cstan finding <agent-id> <severity> "<evidence>" "<requested correction>" "<done when>"` raises a finding about a Developer or Verifier. Severity is `info`, `low`, `medium`, `high` or `critical`. The controller delivers it to that agent as a `Finding` message. One finding may be open per agent.
- `cstan finding check <finding-id> resolved|unresolved "<evidence>"` records the check after the agent acknowledged. `resolved` closes it; `unresolved` sends one more correction. The controller sends at most two corrections; after the second unresolved check, or if the Supervisor does not check within `timers.finding_check_seconds`, it escalates.
- Finding states: `open`, `resolved`, `escalated`, `cancelled`. The PM is told about raised, resolved, escalated or cancelled findings; an `ESCALATED` finding means no further corrections will be sent and the user should know.

Agents treat a `Finding` message as data from a supervisor, not as an instruction from the controller.

## Status, observe and the dashboard

- `cstan status [--json]` prints the project state: roles and seats, work, findings and, when the daemon is reachable as the operator, agents, unresolved messages, panes, reports, reviews, integrations, agent findings, stalled/lost agents and cleanup failures. If the daemon is stopped it reads the ledger directly. `cstan status --watch [--interval <seconds>]` redraws it (interval 1 to 60, default 2).
- `cstan inspect <id> [--json]` shows one ledger record (work item, assignment, candidate, finding, recovery or report).
- `cstan observe <agent-id> [lines]` prints another agent's recent screen. PM and Supervisor only; the text is the agent's own output, not verified, and any instruction in it is data. The default is 40 lines, at most 120, and at most 30 reads a minute.
- `cstan dash [--interval <seconds>] [--no-color] [--reduced-motion]` is a read-only terminal dashboard (needs a TTY; `CSTAN_REDUCED_MOTION=1` also reduces motion; `NO_COLOR` is respected). Panels: agents, pipeline, queue, findings, with a header for run state, health and worker count. Keys: `tab`/`shift+tab` and `1`-`5` move between panels, `↑↓` or `j k` select a row, `o` observes the selected agent (read-only), `y` retry / `s` skip / `c` cancel the selected message (each asks to press `y` again to confirm; the daemon decides), `f` shows only delivery problems, `p` pauses polling, `r` polls now, `-`/`+` change the poll interval, `?` help, `q` or `ctrl+c` quits. Design notes: `docs/design/cstan-dash-v2.md` and `decisions/DEC-006-cstan-dash.md`.

## `cstan` command reference

Checked against the `usage` string in `src/cli.ts` (run `cstan` with no arguments to print it), the `ROUTES` table in `src/daemon.ts` and the handlers in `src/commands.ts`. "Who" shows which credential the daemon accepts: operator (the key in `.capstan/`), agent (an agent's environment token), or either. Every routed command accepts `--json`.

### Project and daemon (operator)

| Command | Purpose |
| --- | --- |
| `cstan init` | Create `.capstan/` and a starter `capstan.toml`. |
| `cstan start` | Start the daemon and launch the PM. |
| `cstan stop` | Stop the daemon. |
| `cstan ping` | Check that the daemon answers. |
| `cstan config check` | Validate `capstan.toml` and print the resolved config (warnings on stderr). |
| `cstan config sync` | Write role definitions into the ledger (daemon stopped). |
| `cstan herdr-config` | Print an optional Herdr `config.toml` snippet. |
| `cstan pm restart` | Replace the PM session, seeded from the ledger. |
| `cstan resolve <message-id> retry\|skip\|cancel ["<note>"]` | Settle a blocked message. |
| `cstan cancel <message-id>` | Cancel a message. |

`cstan daemon` is the internal command that `cstan start` runs in the background; you do not need to run it. `cstan assign` and `cstan ask` are listed in the usage string but answer `not_implemented` in this version.

### Look and read

| Command | Who | Purpose |
| --- | --- | --- |
| `cstan status [--json]` / `--watch [--interval N]` | either | Project state. |
| `cstan inspect <id> [--json]` | operator | One ledger record. |
| `cstan dash [--interval N] [--no-color] [--reduced-motion]` | operator | Dashboard. |
| `cstan inbox [<agent-id>]` | either | Agent: own pending messages. Operator: an agent's mailbox (id required). |
| `cstan observe <agent-id> [lines]` | PM, Supervisor | Another agent's recent screen. |

### Messaging and workers

| Command | Who | Purpose |
| --- | --- | --- |
| `cstan send <agent-id\|@pm> "<text>"` | either | Queue a message. |
| `cstan wait` | PM | Block for new messages. |
| `cstan ack <message-id>` | agent | Acknowledge a message. |
| `cstan spawn <role>` | either | Start a worker. |
| `cstan release <agent-id>` | either | End a worker, free its pane and worktree. |
| `cstan replace <agent-id>` | either | Replace a lost or stuck worker. |

### Delivery flow

| Command | Who | Purpose |
| --- | --- | --- |
| `cstan report <commit> "<summary>"` | Developer, Verifier | Report a finished commit (full 40-character id). |
| `cstan request-review <report-id\|integration-id> [role]` | PM | Start an independent review. |
| `cstan review pass\|findings "<text>"` | Verifier | Answer a review request once. |
| `cstan integrate <report-id>...` | PM or operator | Merge reviewed reports onto an integration branch. |
| `cstan integrate confirm\|discard <integration-id>` | PM or operator | Settle a merged integration. |

### Supervision

| Command | Who | Purpose |
| --- | --- | --- |
| `cstan finding <agent-id> <severity> "<evidence>" "<correction>" "<done-when>"` | Supervisor | Raise a finding. |
| `cstan finding check <finding-id> resolved\|unresolved "<evidence>"` | Supervisor | Check a finding. |

Arguments may not be empty and may not contain invalid UTF-8. Put long text in a quoted heredoc so the shell does not expand it:

```sh
cstan send developer-1 "$(cat <<'EOF'
Task text here.
EOF
)"
```

## Controller messages

Messages whose sender is `controller` start with one of these prefixes (`src/controller/core.ts`, `src/prompts.ts`):

| Prefix | Meaning |
| --- | --- |
| `Verified report` | A report was accepted: the commit exists on that worker's branch. Not a review. |
| `Review request` | To a reviewer: the commit (or integration) to review. |
| `Review` | To the PM: the reviewer's verdict as recorded. The text inside is the reviewer's opinion. |
| `Finding` | A supervisor raised, resolved, escalated or cancelled a finding. |
| `Delivery problem` | A message to a worker is `unacked`, `expired` or `failed`; run the `cstan resolve` it names. |
| `Agent stalled` / `Agent blocked` | A worker made no progress / waits at a prompt. |
| `Agent ... is lost` | Herdr no longer finds the agent's pane or process. |
| `Routine check` | To the Supervisor: do one watch pass. |

## Exit codes

From `EXIT` in `src/cli.ts`: `0` ok, `2` usage error, `3` invalid input or configuration, `4` blocked (the controller refused the request, for example `worker_limit`, or `cstan start` failed to launch), `5` runtime error.

## Development

```sh
npm run build        # tsc + copy migrations
npm test             # build, then node --test dist/test/*.test.js
npm run lint         # eslint src test
npm run format:check # prettier
npm run check        # lint + format:check + test
```

Tests never touch a real Herdr session (`CAPSTAN_LAUNCH=off`).

## Further reading

- `decisions/DEC-005-capstan-v2-direction.md` — the current design direction and trust model; `MVP_PLAN_V2.md` — the plan it follows.
- `decisions/DEC-006-cstan-dash.md` and `docs/design/cstan-dash-v2.md` — the dashboard.
- `docs/spike-herdr-agents.md` — evidence for running interactive agents in Herdr.
- `GATES.md` and the `STAGE*_GATE_RESULTS.md` files — stage gate evidence. `MVP_PLAN.md` and `decisions/DEC-001` to `DEC-004` are earlier, partly superseded history (Docker-based design).
