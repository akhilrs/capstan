# Configuration reference

The full reference for `capstan.toml` and the per-project files. Back to the [README](../../README.md).

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

`cstan init` writes a short starter of this file ([The starter file](#the-starter-file)); every key, default and range is here. Keys are checked strictly: an unknown key is an error (`cstan config check` shows errors and warnings). The authoritative schema is `rust/crates/config/src/types.rs`.

| Table                                                                   | Keys                                                                                                                                                                                                                                                                                                                                   | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ----------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| top level                                                               | `schema_version` (1), `herdr_session`                                                                                                                                                                                                                                                                                                  | Herdr session name; default `default`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[project]`                                                             | `name`                                                                                                                                                                                                                                                                                                                                 | Must match the initialized project name.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `[limits]`                                                              | `max_workers`                                                                                                                                                                                                                                                                                                                          | Default 3, at most 16.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[ledger]`                                                              | `keep_migration_backups`                                                                                                                                                                                                                                                                                                               | Integer 1 to 50, default 3. Before each schema migration the controller copies the ledger to `.capstan/state/controller.sqlite.pre-v<N>-<timestamp>.sqlite`. After a successful migration only the newest this-many such backups (by schema version, then timestamp) are kept and older ones are deleted. Only files with exactly that name are deleted; a failed delete is logged to the daemon's stderr and never fails startup.                                                                                                                                                                                                                                                                                                                                                                              |
|                                                                         | `retention_days`                                                                                                                                                                                                                                                                                                                       | Integer 0 to 3650, default 30. The days the controller keeps the stored result of each mutation request, which is what a replay of that request returns. About five minutes after the daemon starts, and then every hour, it deletes the older rows in batches of at most 1000. `0` turns retention off. `controller_events` are never deleted. **The default is on:** the first daemon start after an upgrade deletes the rows older than 30 days, and that cannot be undone; to keep them, set `retention_days = 0` before upgrading. After a row is deleted its request can no longer be replayed (see "Ledger retention" in docs/daemon.md). The file does not shrink; run `VACUUM` yourself if you want it to.                                                                                             |
| `[layout]`                                                              | `spawn` (`tab` or `pane`), `split` (`auto`, `right`, `down`), `pm_width_percent`, `min_pane_columns`, `min_pane_rows`                                                                                                                                                                                                                  | Starter file sets `spawn = "pane"`, `pm_width_percent = 60`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[worktree]`                                                            | `setup`, `setup_timeout_seconds`, `teardown`, `teardown_timeout_seconds`                                                                                                                                                                                                                                                               | Optional. `setup` is a shell command run once per new worktree with `sh -c`, the worktree as the working directory, e.g. `setup = "make deps"`, whatever your project needs to be ready in a fresh checkout. It applies to workers, replacements and the architect, not the PM. `setup_timeout_seconds` is 1 to 3600, default 600, and is refused without `setup`. A failing or timed-out setup aborts the spawn. The setup command runs with the same filtered environment as agents (the default allowlist plus `[env] pass`), not the daemon's full environment. `setup` is trusted project config, like the other commands in `capstan.toml`: anyone who can edit the file can run commands. See [Worktree teardown](#worktree-teardown) for `teardown`.                                                    |
| `[cleanup]`                                                             | `branches`, `worktrees`, `include_unlanded`                                                                                                                                                                                                                                                                                            | Optional. `branches` is `"auto"`, `"ask"` or `"off"` (default `"ask"`). `worktrees` is `"release"` or `"keep"` (default `"release"`). `include_unlanded` is true or false (default false). See [Cleanup](#cleanup).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[remote]` | `merge`, `merge_branch`, `push`, `push_url`, `messages_per_hour` | Optional. Off by default. See [Remote](#remote). |
| `[supervision]`                                                         | `enabled`, `check_seconds`                                                                                                                                                                                                                                                                                                             | While workers are active the controller keeps one Supervisor running and sends it a routine check (default every 300 s). A Supervisor uses model usage; set `enabled = false` to turn it off.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `[liveness]`                                                            | `enabled`, `check_seconds`, `confirm_checks`, `start_grace_seconds`                                                                                                                                                                                                                                                                    | Optional. See [Liveness](#liveness).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `[messages]`                                                            | `wake`, `send_default`, `info_wake_after_seconds`, `auto_ack_info`, `notice_class`                                                                                                                                                                                                                                                     | Optional. How messages are classed and when info wakes the PM. See [Messages](#messages).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `[defaults]`, `[defaults.PM]`, `.Supervisor`, `.Developer`, `.Verifier` | `model`, `permission_mode`                                                                                                                                                                                                                                                                                                             | Used when a role sets none. `permission_mode` is one of `default`, `acceptEdits`, `plan`, `auto`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `[cache]`                                                               | `presets`, `detect`, `root`, `scope`, `max_size_gb`, `max_age_days`, `rust`, `python`, `node`, `go`, `entries`, `setup_reuse`                                                                                                                                                                                                          | Optional. Shared tool caches for agents. See [Cache](#cache).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `[env]`                                                                 | `pass`                                                                                                                                                                                                                                                                                                                                 | Extra environment variable names copied from where the daemon started into every agent.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `[hosts.<name>]`                                                        | `kind` (`claude`, `codex`, `omp`, `pi`), `command`, `model`, `permission_mode`, `shell_command_timeout_seconds`, `wait_timeout_seconds`                                                                                                                                                                                                | `model` and `permission_mode` are the defaults of every role on the host. A role resolves its model as role > host > `[defaults.<Kind>]` > `[defaults]`, and its permission mode the same way; with none set the mode is `default` on a `claude` host and `auto` on `codex`, `omp` and `pi`. On a non-`claude` host `permission_mode` must be `acceptEdits` or `auto`. `codex`, `omp` and `pi` hosts run unattended with full access; `cstan config check` warns about it. Writing `model` or `permission_mode` on a host changes the `configHash` of the roles on that host, so they are synced again once.                                                                                                                                                                                                    |
| `[roles.<name>]`                                                        | `kind` (`PM`, `Developer`, `Verifier`, `Supervisor`), `host`, `model`, `permission_mode`, `allow`, `deny`, `hooks` (`off` or `inherit`), `prompt` or `prompt_file`, `max_active`                                                                                                                                                       | Role names match `^[a-z][a-z0-9-]{0,31}$` and are limited to 16 characters so the agent name with its project prefix fits Herdr's 32-character limit. A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, unless the role sets `deny` itself. On a `codex`, `omp` or `pi` host `allow`, `deny` and `mcp` are ignored with a warning, and `hooks = "inherit"` draws a warning because it has no effect there. The `default` and `plan` permission modes, `PM` and `Supervisor` kinds, and the `architect`, `operator` and `researcher` roles stay errors on those hosts. `cstan config check` warns when such a role has no model, or takes its model from `[defaults]`. |
| `[architect]`                                                           | `enabled`, `role`, `plan_review` (`high_risk`, `always`, `never`), `reviewer_role`, `max_packages` (1 to 20, default 8), `max_review_rounds` (integer 1 to 20, default 5; the design-round limit of a plan review), `plan_amend` (boolean, default true; allows `cstan plan amend`), `count_toward_worker_limit`, `high_risk_triggers` | Optional planning role; see [Planned work](workflow.md#planned-work-and-the-architect). The role must be a Developer role on a `claude` host; `reviewer_role` must be a Verifier role.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[operator]`                                                            | see the [Operator reference](operator.md)                                                                                                                                                                                                                                                                                              | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[prompt_relay]`                                                        | `enabled`, `capture_ttl_seconds` (60 to 3600, default 600)                                                                                                                                                                                                                                                                             | Optional; lets the PM show you a blocked worker's permission prompt and type the answer you pick. See [Prompt relay](workflow.md#prompt-relay).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[nexora]`                                                              | `track` (`ask`, `always`, `never`), `default_action` (`create`, `link`, `none`)                                                                                                                                                                                                                                                        | Policy only: whether the PM mirrors work into Nexora. Connection details stay in `.nexora.toml`, which Capstan never reads. See [Nexora tracking](workflow.md#nexora-tracking).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[researcher]`, `[mcp_servers.<name>]`                                  | see the [Researcher reference](researcher.md)                                                                                                                                                                                                                                                                                          | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[notifications]`                                                       | `herdr`, `fallback`, `pm_stale_minutes`                                                                                                                                                                                                                                                                                                | At least one of `herdr` and `fallback` must be true. `pm_stale_minutes` (1 to 1440, default 10): once the PM's oldest pending message has waited this long, `cstan status` and the dashboard show a stale-mail banner. The PM wake backoff starts at `timers.pm_wake_interval_seconds`, doubles and is capped by a constant of 1800 s that is not configurable.                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[timers]`                                                              | `max_deferral_seconds`, `max_busy_deferral_seconds`, `pm_ack_timeout_seconds`, `pm_notify_after_seconds`, `notify_interval_seconds`, `stall_after_seconds`, `worker_ack_timeout_seconds`, `finding_check_seconds`, `pm_wake_after_seconds`, `pm_wake_interval_seconds`                                                                 | Defaults and ranges are in `TIMER_DEFAULTS`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

There is no `[daemon]` table any more: Rust is the only daemon. An old `implementation = "rust"` still loads, with a warning; `"node"` is an error (see [docs/daemon.md](../daemon.md)).

The starter file defines these roles, in this order: `pm` (PM), `supervisor` (Supervisor), `architect`, `developer`, `designer`, `operator` and `researcher` (Developer), and `reviewer` and `tester` (Verifier), all on a `claude` host. The reviewer takes `claude-opus-5-5`; the tester takes the `[defaults.Verifier]` model. The starter enables `[architect]`, `[operator]`, `[researcher]` and `[prompt_relay]`. Developer roles deny `git push`; the reviewer and supervisor also deny file-writing tools. Each agent's system prompt is a built-in command reference for its kind (`rust/crates/config/src/prompts.rs`) followed by the role's own prompt: the `prompt` text, or the contents of its `prompt_file`. The `designer` role has a built-in prompt when it sets none; see [The designer role](#the-designer-role).

### The starter file

`cstan init` writes the sections in this order, with only a few comments:

1. `schema_version`, then `[project]` (commented), `[limits]`, `[layout]`, `[notifications]`.
2. The agents' switches: `[supervision]`, `[architect]`, `[operator]`, `[researcher]`, `[prompt_relay]`, then `[nexora]` (commented: it needs a `.nexora.toml`).
3. `[worktree]` and `[env]` (commented examples), `[hosts.*]`, `[defaults.*]`, `[mcp_servers.*]`.
4. All `[roles.*]`: `pm`, `supervisor`, `architect`, `developer`, `designer`, `reviewer`, `tester`, `operator`, `researcher`, then the commented `external-review` example.
5. `[cache]` and `[verify]` (commented examples; both are project-specific).

The sections whose switch is on (`architect`, `operator`, `researcher`, `prompt_relay`) have the roles they need defined. A missing section keeps its code default, so an existing project keeps what its own file says. Add any other table from the lists below. To make `permission_mode = "auto"` the default, set it in a `[defaults.<Kind>]` table.

Defaults and ranges of keys the starter does not show:

| Key | Default | Range / values |
| --- | ------- | -------------- |
| `herdr_session` | `"default"` | The Herdr session Capstan runs its panes in. |
| `[project] name` | the directory name | Must match the name `cstan init` recorded. |
| `[ledger] keep_migration_backups` | 3 | 1 to 50: state backups kept after an upgrade. |
| `[ledger] retention_days` | 30 | 0 to 3650: days a mutation request is kept for replay; 0 keeps all. |
| `[layout] min_pane_columns`, `min_pane_rows` | 60, 12 | 1 to 500, 1 to 200. |
| `[notifications] herdr`, `fallback` | true, true | Send to Herdr; also write `.capstan/state/notifications.jsonl`. They must not both be false. `pm_stale_minutes` (default 10, 1 to 1440): a message for the PM unanswered this long (pulled but not acknowledged counts) raises one "stale" notification. |
| `[hosts.<name>] command` | the kind | Executable name or path. |
| `[hosts.<name>] shell_command_timeout_seconds`, `wait_timeout_seconds` | 120, 90 | 1 to 3600 each; `wait_timeout_seconds` stays below the shell timeout. |
| `[hosts.<name>] permission_mode` | none | `default`, `acceptEdits`, `plan`, `auto` on a claude host; `acceptEdits` or `auto` on codex, omp and pi. |
| `[worktree] setup`, `setup_timeout_seconds` | none, 600 | A shell command (at most 1000 characters) and 1 to 3600; the timeout needs `setup`. Both `setup` and `teardown` run in the project root with `sh -c` and the filtered environment; keep them idempotent. |
| `[env] pass` | none | Names copied from where `cstan start` runs (one-line values only); a name not set there is reported when agents launch. |
| `[timers] max_deferral_seconds` | 120 | 1 to 3600. |
| `[timers] max_busy_deferral_seconds` | 3600 | 60 to 86400. |
| `[timers] pm_ack_timeout_seconds` | 600 | 1 to 86400. |
| `[timers] pm_notify_after_seconds` | 300 | 1 to 86400. |
| `[timers] notify_interval_seconds` | 600 | 1 to 86400. |
| `[timers] stall_after_seconds` | 900 | 1 to 86400. |
| `[timers] worker_ack_timeout_seconds` | 600 | 1 to 86400. |
| `[timers] finding_check_seconds` | 1800 | 60 to 86400. |
| `[timers] pm_wake_after_seconds` | 20 | 0 to 3600. |
| `[timers] pm_wake_interval_seconds` | 120 | 10 to 3600. |
| `[supervision] enabled`, `check_seconds` | true, 300 | The controller keeps one Supervisor running while workers are active; a Supervisor is a Claude session and uses usage. |
| `[roles.<name>] hooks` | none | `off` or `inherit`: whether the role runs your own Claude Code hooks (claude hosts only). |
| `[roles.<name>] max_active` | unlimited | See [`max_active`](#max_active). |
| `[mcp_servers.<name>]` | none | The name becomes the `mcp__<name>__` tool prefix. Pin the package version: an unpinned package (`@latest` or no `@version`) is warned about. |
| `[architect] high_risk_triggers` | `["schema or migrations", "security or auth", "public contracts or wire formats", "cross-cutting changes"]` | Plans that match one are reviewed when `plan_review = "high_risk"`. |
| `[nexora] track`, `default_action` | `"ask"`, `"create"` | Policy only; connection details stay in `.nexora.toml`, which Capstan never reads. `"ask"` shows the PM's intake picker, `"always"` applies `default_action` without asking, `"never"` removes every Nexora instruction from the PM prompt. |
| `[prompt_relay] enabled`, `capture_ttl_seconds` | false, 600 | While false, nothing about the relay reaches an agent and both `cstan prompt` subcommands are unavailable. 60 to 3600. |

The optional Codex, OMP and pi hosts run unattended with full access and without a sandbox: nothing stops a push or an edit outside the worktree, and `cstan config check` warns about every such role. PM, Supervisor, architect, operator and researcher roles stay on a claude host. `codex`, `omp` and `pi` ignore `allow`, `deny`, `mcp` and `hooks` with a warning. See [Running a worker on Codex, OMP or pi](#running-a-worker-on-codex-omp-or-pi).

### `max_active`

`roles.<name>.max_active` is the most agents of that role that may be active at once. It is an integer of 1 or more. When it is absent, a Supervisor role is limited to 1 and every other role is unlimited. A zero, a negative number or a value that is not an integer is a configuration error that names `roles.<name>.max_active`, and so is the key on a PM role, because a project has exactly one PM. The limit applies to each role on its own and lies on top of `limits.max_workers`, which caps all workers together: a spawn must stay under both. A change to `max_active` changes the role's `configHash`, so the role is synced again on the next start (the daemon log shows it).

### Liveness

`[liveness]` configures the check that finds an agent whose session has died. All keys are optional.

| Key | Default | Range | Meaning |
| --- | --- | --- | --- |
| `enabled` | `true` | boolean | Turns the check off when `false`. |
| `check_seconds` | `30` | 10 to 3600 | Seconds between checks. |
| `confirm_checks` | `3` | 2 to 100 | Consecutive failed checks needed before an agent is declared dead. One failed check never counts. |
| `start_grace_seconds` | `120` | 30 to 3600 | Seconds after an agent starts during which it is not checked. |
| `stall_child_max_seconds` | `3600` | 60 to 86400 | The longest a running tool process (a build or test run) keeps a silent agent from being reported as stalled. After that the stall notice is raised, and the daemon log records how many minutes the process has run. |

The last two keys guard against false positives: a session that is slow to answer, or still starting, is not declared dead. A value out of range is a configuration error that names the key.

### Messages

`[messages]` controls how messages between agents are classed and when an info message wakes the PM. Every key is optional; without the table the defaults below apply.

A message is either *action* (it needs the recipient to act or answer) or *info* (a progress note, an acknowledgement or an FYI). Agents send info with `cstan send --info`. An action message wakes the PM as before. Info does not: the PM sees unread info as a digest the next time it reads its inbox, and the digest acknowledges what it shows.

| Key | Default | Values | Meaning |
| --- | --- | --- | --- |
| `wake` | `"action"` | `"action"`, `"all"` | Which messages wake an idle PM. `"all"` wakes it for info too, as before this table existed. |
| `send_default` | `"action"` | `"action"`, `"info"` | The class of a `cstan send` that names neither `--action` nor `--info`. |
| `info_wake_after_seconds` | `600` | integer 0 to 604800 | The PM is woken for info that has stayed unread this long. `0` wakes it at once. |
| `auto_ack_info` | `true` | boolean | The digest acknowledges the info it shows. When `false`, the PM acknowledges info itself. |
| `notice_class` | empty | table of notice kind to `"action"` or `"info"` | Changes the class of a system notice kind. Only the kinds marked overridable below may be changed. |

`cstan config check` refuses a value outside these, an unknown key, an unknown notice kind and a kind that is not overridable, with an error that names the key (for example `messages.notice_class.delivery_problem`).

The notice kinds, their built-in class and whether `notice_class` may change them:

| Kind | Built-in class | Overridable |
| --- | --- | --- |
| `finding_raised` | action | no |
| `finding_escalated` | action | no |
| `finding_intervention` | action | no |
| `finding_resolved` | info | yes |
| `finding_cancelled` | info | yes |
| `cleanup_offer` | action | no |
| `cleanup_done` | info | yes |
| `report_verified` | action | no |
| `review_request` | action | no |
| `review_verdict` | action | no |
| `plan_notice` | action | no |
| `plan_review_failed` | action | no |
| `plan_handover` | action | no |
| `architect_changed` | info | yes |
| `work_package` | action | no |
| `work_package_cancelled` | action | no |
| `delivery_problem` | action | no |
| `agent_stalled` | action | no |
| `agent_blocked` | action | no |
| `agent_lost` | action | no |
| `input_blocked` | action | no |
| `integration_conflict` | action | no |
| `supervision_check` | action | no |
| `pause_notice` | action | yes |
| `operator_proposal` | action | no |
| `operator_proposal_update` | action | yes |
| `operator_grant_update` | action | yes |
| `operator_run_update` | action | yes |
| `restart_result` | info | yes |
| `operator_notice` | info | yes |
| `operator_message` | action | no |
| `auto_approved` | info | yes |
| `auto_answered` | info | yes |
| `approval_window` | action | no |
| `approval_floor_hit` | action | no |

A notice kind that is not overridable is always an action message, so a problem that needs the PM cannot be hidden in the digest.

Upgrade note: this table is part of the configuration hash, and the ledger schema that carries message classes is 40. An older binary refuses a ledger at schema 40. To go back, stop the daemon, restore the backup the migration made (`.capstan/state/controller.sqlite.pre-v<N>-<timestamp>.sqlite`, see `[ledger]`) over `controller.sqlite`, and start the older binary.

### Approval windows (`[operator]`)

Three `[operator]` keys control auto-approval windows. They change nothing outside a window; with no window open every prompt waits for a human as before.

- `approval_floor` (list of strings): what a window never answers. Absent means the built-in floor, in this order: `git push --force`, `git push -f`, `git push --force-with-lease`, `git push --force-if-includes`, `git push --delete`, `git push -d`, `git push --mirror`, `git push --prune`, `@push-rewrite-refspec`, `@delete-outside-project`, `@write-outside-project`, `@controller-restart`. `[]` is accepted with a warning and leaves no floor.
- `auto_window_max_minutes` (default `240`, 1 to `480`): the longest a window lasts. When absent, a written `timed_grant_max_minutes` is used (with its existing 1 to 480 check).
- `auto_window_tool_prompts` (default `true`): whether a window also answers non-file tool prompts. Web fetch, MCP and other non-file tool prompts are answered without a floor check.

An entry is either one of four predicates or a command prefix:

- `@push-rewrite-refspec`: a `git push` whose refspec rewrites a ref (a `+` or leading `:` refspec).
- `@delete-outside-project`: a delete whose target is outside the project root.
- `@write-outside-project`: a write whose target is outside the project root.
- `@controller-restart`: a command that restarts the controller.

A command prefix is 1 to 8 words separated by single spaces, using only letters, digits and `_ . / : = @ % + , -`. It may not start with a variable assignment, and its first word may not be an option. Any other entry starting with `@` is refused. A bad entry makes the configuration fail to load and the message names `operator.approval_floor[i]`.

How a command is matched: it is split into tokens, a leading `cd <dir> &&` is followed so the rest is checked against the directory it names, and an entry matches when its words are a whole-word prefix of the command's tokens (a short option such as `-f` also matches inside a combined group like `-fu`). Paths are compared lexically: `..` and `.` are resolved by text and symlinks are not followed.

The floor is not a sandbox; scripts and shells are approved unchecked during a window.

The JSON output of `cstan config check` prints `approvalFloor`, `autoWindowMaxMinutes` and `autoWindowToolPrompts` only when at least one of the three is written.

#### Removed timed-grant keys

`timed_grant_default_minutes`, `timed_grant_max_minutes` and `timed_safe_commands` are still parsed and validated but no longer have any effect except that `timed_grant_max_minutes` is the fallback for `auto_window_max_minutes`. Each one written gives one warning at daemon start and in `cstan config check`, and none appears in the configuration JSON.

### `prompt_file`

A role may set `prompt_file` instead of `prompt` (never both). The rules:

- The path is relative to the project root and must stay inside it.
- It must be a regular file of at most 64 KiB, valid UTF-8, with no control characters and no text that looks like a credential.
- It is read when the config loads and again when an agent launches, so an edit takes effect at the next launch.
- `cstan config check` prints the file's path and its SHA-256 hash for the role. A missing file fails the load with `prompt_file does not exist`.

### The designer role

The designer's workflow is built into Capstan (`rust/crates/config/src/prompts/designer.md`). A role named `designer` of kind `Developer` that sets neither `prompt` nor `prompt_file` gets that text followed by the design guidance, and `cstan config check` shows its prompt source as `builtin` with the SHA-256 hash of the whole text. The workflow covers the commit-subject and no-attribution rule, Claude Design, a brief, 2 or 3 directions, the build, Playwright verification and an anti-slop standard. The default is keyed on the name and the kind: a `designer` role of another kind, or a Developer role with another name, gets none. The design guidance (`## Design guidance`, ending the built-in prompt) points to topics an agent reads with `cstan design guide <topic>`. A role with its own `prompt` or `prompt_file` does not get the guidance unless `design.guidance = "on"` (see [Design](#design)); to adopt it otherwise, remove that line. To replace the whole prompt, set `prompt` or `prompt_file` on the role.

An older project keeps working unchanged: its `[roles.designer]` still has `prompt_file = "roles/designer.md"`, which resolves to that file with the same text, path and hash as before. To adopt the built-in prompt and its guidance, delete the `prompt_file` line (and the file if you no longer want it); to keep your edits, leave both.

The starter role sets:

```toml
[roles.designer]
mcp = ["playwright"]
allow = ["Bash(git *)", "Skill", "Artifact", "DesignSync", "mcp__playwright", "Bash(python3 -m http.server *)"]
```

`[mcp_servers.playwright]` (pinned `@playwright/mcp`, headless and isolated) is defined in the starter file and shared with the optional researcher. `Artifact` and `DesignSync` exist only when Claude Code on the machine is signed in to claude.ai; without them the prompt tells the designer to say so and report the work as unverified. `DesignSync` is allowed in full, so only the prompt keeps it read-only. Because `mcp` makes Claude Code run with `--strict-mcp-config`, the designer sees only the servers it lists, not your other MCP servers.

To switch Playwright off for the designer, remove the `mcp` line and the `mcp__playwright` allow rule. The prompt then reports the screenshot and contrast checks as unverified. To adopt the workflow in an existing `capstan.toml`, add those lines to `[roles.designer]`, remove its `prompt` and `prompt_file`, and define `[mcp_servers.playwright]`; the workflow text is then the built-in one.

`[researcher]` (`enabled`, `role`, `output_dir`, `user_agent`) turns on the web-research role; `[mcp_servers.<name>]` (`command`, optional `args`) defines an MCP server and `roles.<name>.mcp` lists the servers a `claude`-host role may use. See the [Researcher reference](researcher.md).

### Running a worker on Codex, OMP or pi

A Developer or Verifier role can run on a Codex, OMP or pi host instead of Claude Code. Checked against Codex 0.160.0, OMP 18.3.1 and pi 1.3.0.

1. Declare the host with its `kind` and a `model` your CLI accepts: `[hosts.codex]` with `kind = "codex"`, `[hosts.omp]` with `kind = "omp"`, or `[hosts.pi]` with `kind = "pi"` (pi takes `provider/id`).
2. Switch a role by changing only its `host` line:

   ```toml
   [roles.developer]
   kind = "Developer"
   host = "codex"
   ```

   The role takes the host's `model` and `permission_mode`; `[defaults]` models are Claude models, so a role with no host model gets a warning from `cstan config check`. The permission mode falls back to `auto` on these hosts.
3. Run `cstan config check`. It prints a warning for each unsandboxed role.

**Allowed:** `Developer` and `Verifier` roles, with `permission_mode` `acceptEdits` or `auto`. A role's `allow`, `deny` and `mcp` are ignored with a warning (they are Claude Code options), and `hooks = "inherit"` draws a warning.
**Refused by the configuration loader:** `PM` and `Supervisor` roles, and the `default` and `plan` permission modes. The Architect, Operator and Researcher roles must be on a `claude` host, so they stay on Claude.

**Commit and branch rules apply on every host.** The report check refuses commits that break the Conventional Commits or no-attribution rules whatever host made them (see [Branches, commits and releases](workflow.md#branches-commits-and-releases)). Claude hosts are started with Claude Code's attribution settings off. Codex 0.160.0 and OMP 18.3.1 appear to add no commit trailers (found by a binary string search, not a run); pi was not checked for them, so the report check is the safety net.

**These workers are unsandboxed.** Codex runs with `--sandbox danger-full-access --ask-for-approval never` (its sandbox blocks the daemon socket) and OMP with `--approval-mode yolo`. Nothing blocks a `git push` or an edit outside the worktree; the role prompt asks, the controller does not enforce. Capstan trusts the worktree for Codex with a run-time override, so `~/.codex/config.toml` is not changed.

**pi.** Capstan starts pi (pi.dev, command `pi`) with `--approve --tui-mode regular --offline [--model <model>] --append-system-prompt <prompt file>`. `--approve` trusts the worktree's `.pi` files for that run, so no trust dialog opens and pi's own trust list is not changed; `--tui-mode regular` pins the layout (pi 1.3.0 defaults to fullscreen); `--offline` keeps pi's update notice off the screen. pi has no permission prompts and no sandbox, so there is no approval flag and nothing for the [prompt relay](workflow.md#prompt-relay) to answer. Set the model as `provider/id` (on `[hosts.pi]` or the role), because pi's default provider is `google`. Herdr was seen to report a pi pane's status only with its pi integration installed, so run `herdr integration install pi` first; without it Capstan cannot rely on that status.

### Optional roles

These roles are not in the starter file; add the ones you want to your own `capstan.toml`. Each prompt tells the worker to commit on its own branch and never push or merge.

| Role              | Kind      | Host     | Does                                                                                                                                                                                                                                 |
| ----------------- | --------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `docs-keeper`     | Developer | `pi`     | After an integration or feature, compares the diff with the README and `docs/` and commits the missing or stale Markdown. Never edits code.                                                                                          |
| `security-review` | Verifier  | `claude` | Read-only review of one commit against a security checklist: permission widening, guard or approval bypasses, injection through untrusted text, secret handling, unsafe shell. Ask with `cstan request-review <id> security-review`. |
| `release-manager` | Developer | `claude` | Prepares a release on its branch: version bump, CHANGELOG section, commit-subject check. Denied `git push` and `git tag`; the Operator pushes and tags.                                                                              |
| `host-compat`     | Verifier  | `claude` | Checks a new version of a host CLI (`claude`, `codex`, `omp`, `pi`): recaptures screen fixtures, runs the host parser tests, reports what changed. Denied package-manager and installer commands; the Operator installs.             |
| `external-review` | Verifier  | `codex`  | On-request second opinion from another model family on code, plans or workflow. Ask with `cstan request-review <id> external-review`. Read-only by prompt only: codex runs with full access. Needs `[hosts.codex]` with a `model`.   |
| `integrator`      | Developer | `pi`     | Resolves integration conflicts as a new candidate on its own branch.                                                                                                                                                                 |

`docs-keeper` and `integrator` run on a [pi host](#running-a-worker-on-codex-omp-or-pi) (`[hosts.pi]` with `kind = "pi"`) and take its `model`. Each is one role for one job; switch its host with one `host` line. On pi they are unchecked: pi cannot enforce `allow` or `deny`, so those are ignored, and `cstan config check` warns about each. Their limits (Markdown only; never push, never merge into main, never rewrite another branch) live in the prompt. Run `herdr integration install pi` before a pi role spawns. `security-review`, `release-manager` and `host-compat` need Claude's `deny` lists to block pushes, tags and installs, so they stay on `claude`. `external-review` runs on `codex` with full access, so it is read-only by prompt only; it needs `[hosts.codex]` with a `model` and is used on request with `cstan request-review <id> external-review`. The role name is limited to 16 characters, which is why the review role is `security-review`.

### Worktree teardown

`teardown` is an optional shell command that runs with `sh -c` just before Capstan removes an agent's worktree: on release, when a spawn fails after its worktree was made, and when an agent is replaced. It does not need `setup`. `teardown_timeout_seconds` is 1 to 3600, default 60, and is refused without `teardown`.

- It runs in the project root, not in the worktree.
- Its environment is the same filtered environment as `setup` (the default allowlist plus `[env] pass`), plus `CAPSTAN_WORKTREE_PATH` (the worktree about to be removed) and `CAPSTAN_AGENT_ID`. It gets no `CAPSTAN_TOKEN` and no `CAPSTAN_SOCKET`.
- It does not run while the agent's pane is still open, because the worktree is kept then.
- A failing or timed-out teardown (the process group is killed) is written to the daemon log as `teardown_failed` and never blocks the removal. A start is logged as `teardown_started`.
- If the removal is refused (for example the worktree is dirty) and retried later, the teardown runs again. Write it so it is idempotent.
- It is trusted project config, like `setup`.

Example: remove the codebase-memory-mcp index of each worktree. The index files in `~/.cache/codebase-memory-mcp/` are named after the worktree path with `/` replaced by `-` and the leading `/` dropped, for example `home-me-.herdr-worktrees-proj-proj-developer-3-g1.db`, with `.db-shm`, `.db-wal` and `.db.stage.*` (and `.lock`) siblings. The command names each file exactly, so it cannot match another agent's index (`developer-3` never matches `developer-33`):

```toml
[worktree]
teardown = 'd="$HOME/.cache/codebase-memory-mcp"; n=$(printf %s "${CAPSTAN_WORKTREE_PATH#/}" | tr / -); [ -n "$n" ] && rm -f -- "$d/$n.db" "$d/$n.db-shm" "$d/$n.db-wal" "$d/$n".db.stage.*'
```

Indexes left behind by worktrees removed before you set `teardown` are not cleaned up automatically. To remove them once by hand, list the files in `~/.cache/codebase-memory-mcp/` whose name starts with your worktree base path and whose worktree no longer exists (`git worktree list`), check the list, then delete those files.

Per-project controller files live in `.capstan/`: `project.json` (limits `maxSlices`, `maxRunMs`, `maxDispatches` shown by `cstan status`), `operator.key`, `daemon.log`, and `state/` (`controller.sqlite`, `control.sock`, `notifications.jsonl`, and the newest `controller.sqlite.pre-v*.sqlite` migration backups; see `[ledger]`).

## Cleanup

The `[cleanup]` section sets what Capstan removes after an agent ends. An absent section means the defaults below; unknown keys, values of the wrong type and a `cleanup` that is not a table are refused by `cstan config check`, with a message naming `cleanup.<key>`.

```toml
[cleanup]
branches = "ask"
worktrees = "release"
include_unlanded = false
```

- `branches` (`"auto"`, `"ask"` or `"off"`, default `"ask"`): whether finished agent branches are removed automatically, after asking, or never.
- `worktrees` (`"release"` or `"keep"`, default `"release"`): `release` removes an ended agent's worktree, as before. `keep` closes the pane and ends the agent but leaves the worktree directory and its branch; the daemon log records `worktree_kept_by_config` with `reason` `config_keep`. This is not a failed removal, so `cstan status` does not list it under `cleanupFailed`. With `keep`, only an explicit `cleanup apply` removes a worktree.
- `include_unlanded` (true or false, default false): whether branches that are not merged are candidates for cleanup. Unlanded branches are never removed automatically, whatever this is set to.

To restore a branch that was removed, recreate it from the commit it pointed at: `git branch <name> <sha>`.

## Remote

The `[remote]` section says whether integrated work may be merged into a remote branch and pushed from it. An absent section means the defaults below: nothing is merged or pushed. Unknown keys, values of the wrong type and a `remote` that is not a table are refused by `cstan config check`, with a message naming `remote.<key>`; a refusal never repeats the rejected value.

```toml
[remote]
merge = false
merge_branch = "main"
push = false
push_url = "git@example.com:org/repo.git"
messages_per_hour = 30
```

- `merge` (true or false, default false): whether a merge into the remote branch may be made.
- `merge_branch` (string, default unset): the branch to merge into. Unset means the remote's default branch (origin `HEAD`) at merge time, and the merge is refused when there is none. It is checked whenever it is set and ignored while `merge` is false. A name must follow git's branch rules, or stricter: not empty; no leading `-` or `/`; no `..`, `//` or `@{`; no control character, space, `~`, `^`, `:`, `?`, `*`, `[` or `\`; no trailing `/` or `.`; no component that starts with `.` or ends with `.lock`; not `@` alone and not `HEAD`.
- `push` (true or false, default false): whether the merged branch is pushed. It needs `merge = true` and a `push_url`.
- `push_url` (string, default unset, at most 1024 bytes): where a push goes. It is checked whenever it is set. Accepted: `ssh://`, `https://` or `file://` URLs (the scheme in any case; only `file://` may have an empty host; `ssh://` may carry a port from 1 to 65535 or a bracketed IPv6 host), and the scp-like form `[user@]host:path`. Refused: `http://`, `git://` and any other scheme, a remote helper such as `ext::x`, a password in a URL (`ssh://user:pw@host/r`), a value that starts with `-` or whose host does, whitespace, control characters, `?`, `#` and `%`.
- `messages_per_hour` (1 to 600, default 30): a hard cap on messages per project and per direction over the trailing 60 minutes.

capstan.toml (read from the project root, and re-read at every merge call) is often committed, so a cloned repository can arrive with `[remote]` merge or push enabled and its own `push_url`. Review `[remote]` before running `cstan` in a repository you did not write.

Limits: percent-encoded file paths are not supported. A 2-letter scp host such as `cd:path` is taken as a host, never as a Windows drive; `c:path` is refused as a local path. A bracketed IPv6 host works only in `ssh://` form, not scp-like.

### Remote merge and push safety

Root git calls trust the user's global and system git config, which any program running as the same user can change.

A `push_url` matched by any `url.*.insteadOf` or `url.*.pushInsteadOf` rule in the user's git config is not pushed (`push_url_rewritten`), so the push goes exactly to the validated URL and its later check reads the same one.

Root calls forward no desktop session variables (a credential helper that needs `DBUS_SESSION_BUS_ADDRESS` or `XDG_RUNTIME_DIR` fails) but do forward `http_proxy` and the like.

The tables below list the fixed values; each is one column, one row per value.

### Refused local config keys

A root git call refuses to run when the repository's local config sets any of these keys (`*` stands for any name).

| Value |
| --- |
| `merge.*.driver` |
| `filter.*.clean` |
| `filter.*.smudge` |
| `filter.*.process` |
| `diff.external` |
| `diff.*.textconv` |
| `diff.*.command` |
| `core.sshCommand` |
| `core.askPass` |
| `core.gitProxy` |
| `core.pager` |
| `core.editor` |
| `core.attributesFile` |
| `core.worktree` |
| `core.bare` |
| `core.alternateRefsCommand` |
| `sequence.editor` |
| `submodule.*.update` |
| `mergetool.*.cmd` |
| `push.recurseSubmodules` |
| `http.proxy` |
| `http.proxyAuthMethod` |
| `http.sslVerify` |
| `http.sslCert` |
| `http.sslKey` |
| `http.sslCAInfo` |
| `http.sslCAPath` |
| `http.sslBackend` |
| `http.pinnedPubkey` |
| `http.followRedirects` |
| `http.extraHeader` |
| `http.cookieFile` |
| `http.curloptResolve` |
| `http.*.proxy` |
| `http.*.proxyAuthMethod` |
| `http.*.sslVerify` |
| `http.*.sslCert` |
| `http.*.sslKey` |
| `http.*.sslCAInfo` |
| `http.*.sslCAPath` |
| `http.*.sslBackend` |
| `http.*.pinnedPubkey` |
| `http.*.followRedirects` |
| `http.*.extraHeader` |
| `http.*.cookieFile` |
| `http.*.curloptResolve` |
| `ssh.variant` |
| `remote.*.proxy` |
| `remote.*.proxyAuthMethod` |
| `credential.*` |
| `gpg.program` |
| `gpg.*.program` |
| `remote.*.receivepack` |
| `remote.*.uploadpack` |
| `remote.*.vcs` |
| `url.*.insteadOf` |
| `url.*.pushInsteadOf` |
| `protocol.*.allow` |
| `include.path` |
| `includeIf.*` |

### Refusal codes

A merge request is refused before it starts with one of these.

| Value |
| --- |
| `unsafe_config` |
| `invalid_branch` |
| `invalid_commit` |
| `wrong_branch` |
| `no_identity` |
| `dirty_tree` |
| `merge_conflict` |
| `git_too_old` |

### Failure reasons

A merge that started and failed reports one of these.

| Value |
| --- |
| `merge_failed` |
| `index_lock_left` |
| `timeout` |
| `tree_mismatch` |
| `no_merge_head` |

### Push reasons

A push that is not made or fails reports one of these.

| Value |
| --- |
| `invalid_branch` |
| `unsafe_config` |
| `no_upstream` |
| `mirror` |
| `url_mismatch` |
| `push_url_rewritten` |
| `not_fast_forward` |
| `rejected` |
| `timeout` |
| `unreachable` |

## Cache

The `[cache]` section gives agents shared compiler and package-manager caches, so a new worktree does not start cold. An absent section means no caches and no change for the project. Unknown keys, values of the wrong type and invalid entries are refused by `cstan config check`, with a message naming the key (`cache.entries.<name>.dirs.<ENV>`).

```toml
[cache]
presets = ["rust", "python", "node", "go"]
detect = false
root = "~/.cache/capstan"
scope = "project"
max_size_gb = 20
max_age_days = 30

[cache.rust]
mode = "sccache"

[cache.entries.bundler]
dirs = { BUNDLE_PATH = "bundle" }
set = { BUNDLE_JOBS = "4" }
requires = "bundle"
prune = ["bundle clean"]

[cache.setup_reuse]
key_files = ["Cargo.lock"]
paths = ["target"]
```

| Key | Meaning |
| --- | --- |
| `presets` | Built-in entries to turn on: `rust`, `python`, `node`, `go`. A listed preset is always on. |
| `detect` | `true` also turns on the presets whose marker files are in the project root or one directory below it (skipping `.git`, `node_modules` and `target`). It never turns a listed preset off. Default `false`. The config only stores the flag; the project is scanned when the daemon loads the config and by the `cstan cache` commands, never when `capstan.toml` is parsed. |
| `root` | Where the caches live: an absolute path or one starting with `~/`, without `..`, and not inside the project root. |
| `scope` | `"project"` (default) or `"user"`: whether the caches belong to this project or are shared by every project that uses the same `root`. |
| `max_size_gb` | The total size cap, a number from 0.1 to 10000. Default 20 when `[cache]` is present. |
| `max_age_days` | Entries unused for longer are pruned, an integer from 1 to 3650. Default 30. |
| `[cache.<preset>]` | Settings of a preset: `max_size_gb` for any, and `mode` and `incremental` for `rust`. The table is allowed only for a listed preset (or when `detect` is true). |
| `[cache.entries.<name>]` | Your own entry; see below. |
| `[cache.setup_reuse]` | `key_files` and `paths`, both non-empty lists of relative paths without `..`: the files that identify a worktree's setup result and the directories to reuse when they are unchanged. |

An entry's `max_size_gb` may not exceed the total, and the entries' sizes may not add up to more than the total.

### Presets

Each `dirs` pair is `ENV = subdirectory` of the entry's cache directory; `set` pairs are fixed values.

| Preset | Variables | Needs | Also |
| --- | --- | --- | --- |
| `rust`, mode `sccache` (default) | `SCCACHE_DIR=sccache`; `RUSTC_WRAPPER=sccache`, `SCCACHE_CACHE_SIZE` from the cap (the preset's `max_size_gb`, or the total); `CARGO_INCREMENTAL=0` unless `incremental = true` | `sccache` on PATH | stops the server with `sccache --stop-server` before the cache is cleared |
| `rust`, mode `shared-build-dir` | `CARGO_BUILD_BUILD_DIR=cargo-build` | `cargo` | |
| `rust`, mode `off` | none | | |
| `python` | `PIP_CACHE_DIR=pip`, `UV_CACHE_DIR=uv`, `POETRY_CACHE_DIR=poetry` | | prune: `uv cache prune` |
| `node` | `NPM_CONFIG_CACHE=npm`, `NPM_CONFIG_STORE_DIR=pnpm-store`, `YARN_CACHE_FOLDER=yarn` | | prune: `npm cache verify`, `pnpm store prune` |
| `go` | `GOCACHE=go-build`, `GOMODCACHE=go-mod` | | |

`[cache.rust] mode` accepts `sccache`, `shared-build-dir` and `off`; anything else is refused.

`[cache.rust] incremental` (default `false`) applies to mode `sccache`: sccache cannot cache incremental crates, so the preset sets `CARGO_INCREMENTAL=0` and the incremental cache (about 3 GB per worktree) is not written. `incremental = true` leaves cargo's own default. `scripts/check.sh` sets the same `CARGO_INCREMENTAL=0` for its cargo stages unless the caller already sets the variable.

Detection markers: `Cargo.toml` (rust); `pyproject.toml`, `requirements*.txt`, `setup.py`, `uv.lock`, `poetry.lock` (python); `package.json` (node); `go.mod` (go).

### Adding your own entry

A project with a tool that has no preset adds `[cache.entries.<name>]` (a lower-case name, not `rust`, `python`, `node` or `go`). Keys:

- `dirs`: a table `ENV = "subdir"`. Each subdirectory is one directory name of letters, digits, `.`, `_` and `-`: no `/`, no `..`, not absolute.
- `set`: a table `ENV = "value"` of fixed values. One line, no credential-shaped text. Use it for non-secret values only.
- `requires`: a program name that must be on the agent's PATH for the entry to apply.
- `prune`: commands run with the entry's environment when the cache is pruned. A command for a tool that is not installed is skipped by the cache store.
- `stop`: commands run before the entry's directories are cleared, for example to stop a background server.
- `max_size_gb`: a cap for this entry (0.1 to 10000).

An entry needs `dirs` or `set`. Variable names must be upper case letters, digits and `_` (at most 64 characters), and are refused when they:

- start with `CAPSTAN_`, or are `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `TERM`, `TMPDIR` or another name Capstan reserves for `[env] pass` (including the `LD_`, `DYLD_`, `GIT_` and `BASH_` prefixes);
- are listed in `[env] pass`;
- contain `TOKEN`, `SECRET`, `KEY`, `PASSWORD`, `PASSWD` or `CREDENTIAL`: caches never carry secrets. A legitimate variable with such a name, for example a license key, goes through `[env] pass`, not `[cache]`;
- are already set by another entry or preset.

## Verify

The `[verify]` section names the project's own gate commands. `cstan verify` runs them in a scratch worktree, in order, and records pass or fail for the exact commit and command list, so a report or a review can rely on a result instead of an agent's word. An absent section means no gate and no change for the project. Unknown keys, values of the wrong type and invalid values are refused by `cstan config check`, with a message naming the key (`verify.max_parallel`).

```toml
[verify]
commands = ["scripts/check.sh rust"]
env = { CI = "1" }
timeout_seconds = 3600
max_parallel = 1
auto = ["report", "integration"]
require = ["review", "integrate"]
output_tail_bytes = 8192
max_log_bytes = 4194304
keep_logs = 20
worktree_hooks = true
reap = true
scratch_dir = "~/.cache/capstan/verify"
```

| Key | Meaning |
| --- | --- |
| `commands` | Required. 1 to 16 non-empty strings of at most 1000 characters, each run with `sh -c` in the scratch worktree, in order; the run stops at the first non-zero exit. |
| `env` | Extra variables for the commands, `{ NAME = "value" }`. A name is checked like `[env] pass`: upper-case letters, digits and underscore, not `CAPSTAN_*`, and none of the reserved names (`PATH`, `HOME`, `SHELL`, ...) or prefixes (`LD_`, `DYLD_`, `GIT_`, `BASH_`). Values must not look like credentials. |
| `timeout_seconds` | Limit for the whole run, 60 to 14400. Default 3600. |
| `max_parallel` | Runs that may be active at once, 1 to 8. Default 1. |
| `auto` | Any of `"report"`, `"integration"`: verify automatically when a developer reports, or when an integration is made. Default `[]`. |
| `require` | Any of `"review"`, `"integrate"`: refuse `cstan request-review` or `cstan integrate` with `verify_required` unless a passed run matches the commit and the commands. Default `[]`. |
| `output_tail_bytes` | How much of the end of the output is kept in the result and shown in briefs, 1024 to 12288. Default 8192. |
| `max_log_bytes` | Cap on the full log of one run, 65536 to 67108864. Default 4194304. |
| `keep_logs` | Number of run logs kept, 1 to 500. Default 20. |
| `worktree_hooks` | `true` (default) runs `[worktree] setup` before the commands and `teardown` after them, in the scratch worktree. |
| `reap` | `true` (default) stops, when a run ends, every process that still carries the run's `CAPSTAN_VERIFY_RUN_ID` in its environment, including a tool daemon (an sccache server, say) the gate started in another session; `false` leaves them running, for tools other agents reuse. The scratch worktree is always removed. After a daemon crash or restart the processes of an interrupted run are always stopped this way, whatever `reap` says, because the ledger records only the first step's process group; the recovery teardown hook gets `CAPSTAN_VERIFY_COMMIT` like the run. |
| `scratch_dir` | Where scratch worktrees are made: an absolute path or one starting with `~/`, without `..`. Default `<cache root>/projects/<project id>/verify`, resolved by the daemon. The daemon makes the directory with a `.capstan-verify` marker file naming the project and refuses a directory that already holds other files and has no marker (`verify_scratch_refused`); it removes only worktrees of this repository that it finds there. |

A passed run is keyed by the commit and by the SHA-256 of the command list (`commands_sha`): changing, adding, removing or reordering a command makes earlier passes stale. A review brief carries a gate line; when it says the gate passed at the reviewed commit, the reviewer does targeted checks and does not rerun the full gate.

### Examples

Rust:

```toml
[verify]
commands = ["scripts/check.sh rust"]
```

Python:

```toml
[verify]
commands = ["pytest -q"]
timeout_seconds = 1800
```

Node:

```toml
[verify]
commands = ["npm test"]
env = { CI = "1" }
```

Go:

```toml
[verify]
commands = ["go vet ./...", "go test ./..."]
```

## Design

The `[design]` section controls the user's approval of a design before the packages that build on it are assigned, and tells the Developer role named `designer` where its design record lives and how it takes guidance. An absent section, and an empty `[design]`, behave as every default below. `cstan config` prints a `design` object only when the section is written: `{ configured, requireApproval }`, and, only when a key after `require_approval` has a value other than its default, also `designRecord`, `designRecordMode`, `guidance`, `guidanceRules`, `guidanceExtra`, `directionsMin`, `directionsMax` and `referenceCollection` (`null` when there is none, else as written). A key written with its default value changes nothing. A value of the wrong type or out of range, and an unknown key, is refused by `cstan config check` with a message naming the key.

| Key                    | Default                                                        | Meaning |
| ---------------------- | -------------------------------------------------------------- | ------- |
| `require_approval`     | `true`                                                         | When true, `cstan plan assign` refuses a package that depends on a design package (`"design": true` in the plan) until the PM has recorded the user's approval of that design package's latest report with `cstan design approve`. When false, assignment ignores design approvals. Plans without design packages are not affected either way. |
| `design_record`        | `"DESIGN.md"`                                                  | The design record: one repo-relative file, or a list of 1 to 8, one per surface. |
| `design_record_mode`   | `"maintain"`                                                   | `maintain`: the designer keeps the record current. `read`: it only reads the record. `off`: there is no record. |
| `guidance`             | `"builtin"`                                                    | `builtin`: the design guidance reaches only the built-in designer prompt, as before. `on`: it also reaches a designer with its own `prompt` or `prompt_file`. `off`: never. |
| `guidance_rules`       | `"extend"`                                                     | `extend`: the guidance rules add to the designer's own rules. `override`: they replace them. |
| `guidance_extra`       | `["docs/design/directions.md", "docs/design/rules.md"]`        | Up to 8 repo-relative files the designer reads when they exist. Capstan never reads them. |
| `directions_min`       | `2`                                                            | 2 to 5: the fewest directions the designer proposes. |
| `directions_max`       | `3`                                                            | 2 to 6, not below `directions_min`: the most directions the designer proposes. |
| `reference_collection` | `""` (none)                                                    | An absolute path or a `~/` path outside the project with reference material for the designer. Capstan never reads it. |

Paths in `design_record` and `guidance_extra` are normalised (`.` segments and repeated slashes are dropped; the order is kept) and need not exist. An entry is refused when it is empty, is not relative to the project (a leading `/` or `~`, or a drive prefix such as `C:`), has a `..` segment (`a..b` is a valid name), has a backslash, a control, format or line-separator character, leading or trailing whitespace, a glob character (`*`, `?`, `[`, `]`), ends with `/`, is longer than 100 bytes, or repeats an earlier entry of its list; a `guidance_extra` entry equal to a `design_record` entry is refused too. `design_record` takes 1 to 8 entries and `guidance_extra` at most 8. A path on disk must stay inside the project: on every load, a symlink on the way to an entry that leads outside the project is refused (`design.design_record[0] must stay inside the project`), including a symlink that a path not yet created would pass through. The designer checks again before it reads or writes a path, because paths can change after a load.

`reference_collection` is checked as text: it must be at most 300 bytes, absolute or starting with `~/`, without a `..` segment, and outside the project and not containing it. A `~/` path is expanded with the `HOME` environment variable, which is read on every load, and is refused on a machine without `HOME` (or with an empty one). Symlinks in the collection path are not resolved, so a symlink into the project is accepted; Capstan never reads the collection, the designer reads it as files. A collection that does not exist is only warned about (`design.reference_collection <path> does not exist`). A designer on a `claude` host needs a read rule such as `Read(<path>/**)` in `[roles.designer].allow` to read a reference collection.

`directions_min` is checked first, then `directions_max`, then the two together after defaults are applied: `directions_min = 4` alone is refused (`design.directions_min (4) is greater than design.directions_max (3); set both`), so a minimum of 4 needs `directions_max` of 4 or more.

When any of these keys is not at its default and no Developer role is named `designer`, `cstan config check` warns `design: no Developer role named designer, so the guidance and record keys change no prompt`.

### What the keys put into the designer's prompt

For a Developer role named `designer`, the keys above add text after the role's own prompt (`prompt_text`). A key at its default adds nothing, so a project without them keeps its prompt, `prompt.hash` and `configHash`.

- Built-in prompt (no `prompt`, no `prompt_file`, or a `prompt_file` that does not exist): the designer text and the guidance, then a `### This project` block when a key other than `guidance` is not at its default. The block lists the values that replace the defaults in the guidance: the number of directions, whether the guidance rules extend or override the designer's own, the project files, the design record paths and mode, the reference collection, and a line that tells the designer to check that a path resolves inside the repository before it reads or writes it. With `guidance = "off"` the prompt is the designer text alone, followed only by the design record lines when a record key is changed.
- Own prompt with `guidance = "on"`: the own text, the guidance, then the block.
- Own prompt with `guidance = "builtin"` or `"off"`: the own text, then only the design record lines when `design_record` or `design_record_mode` is changed; with `design_record_mode = "off"` they tell the designer to ignore the record. The other keys shape the guidance, which this prompt does not get, so with `guidance = "builtin"` and a changed `directions_min`, `directions_max`, `guidance_rules`, `guidance_extra` or `reference_collection`, `cstan config check` warns `design: guidance = "builtin" and roles.designer has its own prompt, so <keys> have no effect; set guidance = "on"`.

The block is at most 3072 bytes. A designer on a Codex host with a large `prompt_file` and `guidance = "on"` gains at most 10 KiB; a prompt over the host's argument limit is refused at spawn with the usual message, and there is no extra cap.

`prompt.source`, `prompt.path` and `prompt.hash` keep describing the role's own text; the built-in prompt's own copy of the guidance stays inside source `builtin` and `prompt.hash`. Text appended after the role's own prompt is described by a `guidance` member of the printed `prompt` object, `{ "parts": [...], "hash": "..." }`, present only when something was appended: `parts` lists, in order, `guidance` (the guidance, for an own prompt with `guidance = "on"`), `project` (the block) and `record` (the design record lines), and `hash` is the SHA-256 of exactly the appended bytes, separators included. The hash is part of the role's `configHash`, so a changed block re-syncs the designer.

The Architect's prompt gets a note when `design_record` or `design_record_mode` is changed: it names the record paths and the mode, and says who owns them (`maintain`: exactly one design package of the plan owns each path it changes; `read`: no package owns a record path; `off`: designers ignore the record). The note is built when the Architect is started and is not part of any `configHash`: a reload does not send it to a running Architect, which sees a change on its next start. Ownership stays a prompt-level rule that the controller does not enforce.

```toml
[design]
require_approval = false
design_record = ["DESIGN.md", "docs/design/mobile.md"]
design_record_mode = "maintain"
directions_min = 4
directions_max = 4
reference_collection = "~/design-references"
```

## Review

The `[review]` section controls whether a plan's reviewer is kept between review requests. An absent section behaves as an empty one, with every default below, except that `cstan config` prints a `review` object only when the section is written. Unknown keys, values of the wrong type and out-of-range values are refused by `cstan config check`, with a message naming the key (`review.idle_minutes`).

```toml
[review]
warm_reviewer = true
idle_minutes = 30
max_reviews = 6
integration = false
```

| Key | Meaning |
| --- | --- |
| `warm_reviewer` | `true` or `false`. Keep a plan's reviewer after its verdict, so the next `cstan request-review` of the same plan reuses it (the architect sees `reviewerReused` in the output). Default `true`: a reused reviewer already has the repository and the plan in context, which saves the start-up time and tokens of a fresh session for every package. With `false`, the controller ends the reviewer after each verdict and every request starts a fresh one, as before this section existed. |
| `idle_minutes` | 5 to 240. How long a kept reviewer may wait for its next request before the controller ends it. Default `30`: long enough to span a package's fix cycle, short enough that an abandoned plan does not hold a session open. |
| `max_reviews` | 1 to 20. How many requests one reviewer answers before it is replaced by a fresh one. Default `6`: a long-lived reviewer's context fills with earlier verdicts, so a bound keeps its judgement of each commit independent. |
| `integration` | `true` or `false`. Whether integration reviews may also reuse the plan's reviewer. Default `false`: an integration review should look at the merged result without the bias of the package reviews. |

Each request is its own task: the reviewer judges the commit it names, treats earlier verdicts as context only, and waits for the next request until the controller ends the session.

## `CAPSTAN_ALLOW_FOREIGN_SOCKET`

Set to `1` in an agent shell to let `cstan` use a `CAPSTAN_SOCKET` that is not the socket of the project (nearest `.capstan/` ancestor) the working directory is in. Without it such a command exits 2 and sends no request. With it, one warning line is printed per command. `cstan inbox --hook` ignores it: on a foreign socket it stays silent and makes no request.
