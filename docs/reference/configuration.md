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
|                                                                         | `cache_mb` | Integer 0 to 1024, default 64. The ledger connection's page cache in MiB (`PRAGMA cache_size`). It is memory the daemon may hold, up to this much, on top of the OS page cache. `0` turns it off and leaves SQLite's own default (about 2 MB). |
|                                                                         | `mmap_mb`  | Integer 0 to 4096, default 256. Memory-mapped reads of the ledger in MiB (`PRAGMA mmap_size`). The mapping shares the OS page cache, so it counts as address space and shared page cache, not as private memory; it is capped at the file size. `0` turns it off. |
|                                                                         | `backup_max_age_days` | Integer 0 to 3650, default 14. Days a pre-migration backup is kept; older ones are removed by the daily ledger maintenance (see `daemon.md`). `0` turns the age limit off and keeps only the newest `keep_migration_backups`. |
| `[daemon]`                                                              | `status_cache_ms` | Integer 0 to 60000, default 2000. How long the daemon serves a cached `status` and event-feed reading. `0` turns both caches off. Consumed by the status-cache package; until it lands the key has no effect. |
|                                                                         | `driver_read_first` | Boolean, default `true`. The driver reads before it mutates and writes only when something changed. `false` keeps the older path that mutates on every tick. Consumed by the driver-reads-first package (`DriverOptions.read_first`); until it lands the key has no effect. |
|                                                                         | `snapshot_reads_per_minute` | Integer 0 to 600, default 0. The most feed snapshot reads per minute; `0` means no limit. Consumed by the feed-snapshot-cache package; until it lands the key has no effect. |
|  | `max_connections` | Integer 8 to 1024, default 64. The most connections the daemon holds open at once. Read at daemon start only. Consumed by the server-admission package. |
|  | `caller_connections` | Integer 1 to `max_connections`, default 32. The most open non-wait connections one caller may hold. Read at daemon start only. Consumed by the server-admission package. |
|  | `wait_connections` | Integer 1 to `max_connections`, default 32. The most open wait-class connections (`wait` and the like) in all. Read at daemon start only. Consumed by the server-admission package. |
|  | `wait_per_caller` | Integer 1 to `wait_connections`, default 4. The most open wait-class connections one caller may hold. Read at daemon start only. Consumed by the server-admission package. |
|  | `repeat_window_seconds` | Integer 0 to 600, default 60. Seconds in which a repeated send is folded into the first one: enough for a retry storm, short enough for a deliberate resend. `0` turns it off. Never above the reply cache's 600 s entry life. Read at daemon start only. Consumed by the reply-cache package. |
|  | `busy_retry_ms` | Integer 50 to 10000, default 500. The retry hint in a `controller_busy` answer. Read at daemon start only. Consumed by the client-retry package. |
| `[layout]`                                                              | `spawn` (`tab` or `pane`), `split` (`auto`, `right`, `down`), `pm_width_percent`, `min_pane_columns`, `min_pane_rows`                                                                                                                                                                                                                  | Starter file sets `spawn = "pane"`, `pm_width_percent = 60`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[worktree]`                                                            | `setup`, `setup_timeout_seconds`, `teardown`, `teardown_timeout_seconds`                                                                                                                                                                                                                                                               | Optional. `setup` is a shell command run once per new worktree with `sh -c`, the worktree as the working directory, e.g. `setup = "make deps"`, whatever your project needs to be ready in a fresh checkout. It applies to workers, replacements and the architect, not the PM. `setup_timeout_seconds` is 1 to 3600, default 600, and is refused without `setup`. A failing or timed-out setup aborts the spawn. The setup command runs with the same filtered environment as agents (the default allowlist plus `[env] pass`), not the daemon's full environment. `setup` is trusted project config, like the other commands in `capstan.toml`: anyone who can edit the file can run commands. See [Worktree teardown](#worktree-teardown) for `teardown`.                                                    |
| `[cleanup]`                                                             | `branches`, `worktrees`, `include_unlanded`                                                                                                                                                                                                                                                                                            | Optional. `branches` is `"auto"`, `"ask"` or `"off"` (default `"ask"`). `worktrees` is `"release"` or `"keep"` (default `"release"`). `include_unlanded` is true or false (default false). See [Cleanup](#cleanup).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[remote]` | `merge`, `merge_branch`, `push`, `push_url`, `messages_per_hour` | Optional. Off by default. See [Remote](#remote). |
| `[impact]` | `areas`, `migrations`, `next_steps` | Optional. How the details recorded at merge group paths. See [Impact](#impact). |
| `[ntfy]` | `enabled`, `server`, `topic`, `token_env`, `token_file`, `user`, `password_env`, `password_file`, `events`, `timeout_seconds`, `retries`, `ca_file`, `title_prefix`, `event` | Optional. Off by default. See [Push notifications (ntfy)](#push-notifications-ntfy). |
| `[supervision]`                                                         | `enabled`, `check_seconds`                                                                                                                                                                                                                                                                                                             | While workers are active the controller keeps one Supervisor running and sends it a routine check (default every 300 s). A Supervisor uses model usage; set `enabled = false` to turn it off.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `[liveness]`                                                            | `enabled`, `check_seconds`, `confirm_checks`, `start_grace_seconds`                                                                                                                                                                                                                                                                    | Optional. See [Liveness](#liveness).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `[messages]`                                                            | `wake`, `send_default`, `info_wake_after_seconds`, `auto_ack_info`, `notice_class`                                                                                                                                                                                                                                                     | Optional. How messages are classed and when info wakes the PM. See [Messages](#messages).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `[defaults]`, `[defaults.PM]`, `.Supervisor`, `.Developer`, `.Verifier` | `model`, `permission_mode`                                                                                                                                                                                                                                                                                                             | Used when a role sets none. `permission_mode` is one of `default`, `acceptEdits`, `plan`, `auto`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `[cache]`                                                               | `presets`, `detect`, `root`, `scope`, `max_size_gb`, `max_age_days`, `rust`, `python`, `node`, `go`, `entries`, `setup_reuse`                                                                                                                                                                                                          | Optional. Shared tool caches for agents. See [Cache](#cache).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `[env]`                                                                 | `pass`                                                                                                                                                                                                                                                                                                                                 | Extra environment variable names copied from where the daemon started into every agent.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `[hosts.<name>]`                                                        | `kind` (`claude`, `codex`, `omp`, `pi`), `command`, `model`, `permission_mode`, `shell_command_timeout_seconds`, `wait_timeout_seconds`                                                                                                                                                                                                | `model` and `permission_mode` are the defaults of every role on the host. A role resolves its model as role > host > `[defaults.<Kind>]` > `[defaults]`, and its permission mode the same way; with none set the mode is `default` on a `claude` host and `auto` on `codex`, `omp` and `pi`. On a non-`claude` host `permission_mode` must be `acceptEdits` or `auto`. `codex`, `omp` and `pi` hosts run unattended with full access; `cstan config check` warns about it. Writing `model` or `permission_mode` on a host changes the `configHash` of the roles on that host, so they are synced again once.                                                                                                                                                                                                    |
| `[roles.<name>]`                                                        | `kind` (`PM`, `Developer`, `Verifier`, `Supervisor`), `host`, `model`, `permission_mode`, `allow`, `deny`, `hooks` (`off` or `inherit`), `memory`, `prompt` or `prompt_file`, `max_active`                                                                                                                                                       | Role names match `^[a-z][a-z0-9-]{0,31}$` and are limited to 16 characters so the agent name with its project prefix fits Herdr's 32-character limit. A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, unless the role sets `deny` itself. On a `codex`, `omp` or `pi` host `allow`, `deny` and `mcp` are ignored with a warning, and `hooks = "inherit"` draws a warning because it has no effect there. The `default` and `plan` permission modes, `PM` and `Supervisor` kinds, and the `architect`, `operator` and `researcher` roles stay errors on those hosts. `cstan config check` warns when such a role has no model, or takes its model from `[defaults]`. |
| `[architect]`                                                           | `enabled`, `role`, `plan_review` (`high_risk`, `always`, `never`), `reviewer_role`, `max_packages` (1 to 20, default 8), `max_review_rounds` (integer 1 to 20, default 5; the design-round limit of a plan review), `plan_amend` (boolean, default true; allows `cstan plan amend`), `count_toward_worker_limit`, `high_risk_triggers` | Optional planning role; see [Planned work](workflow.md#planned-work-and-the-architect). The role must be a Developer role on a `claude` host; `reviewer_role` must be a Verifier role.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[operator]`                                                            | see the [Operator reference](operator.md)                                                                                                                                                                                                                                                                                              | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[prompt_relay]`                                                        | `enabled`, `capture_ttl_seconds` (60 to 3600, default 600)                                                                                                                                                                                                                                                                             | Optional; lets the PM show you a blocked worker's permission prompt and type the answer you pick. See [Prompt relay](workflow.md#prompt-relay).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[nexora]`                                                              | `track` (`ask`, `always`, `never`), `default_action` (`create`, `link`, `none`)                                                                                                                                                                                                                                                        | Policy only: whether the PM mirrors work into Nexora. Connection details stay in `.nexora.toml`, which Capstan never reads. See [Nexora tracking](workflow.md#nexora-tracking).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[researcher]`, `[mcp_servers.<name>]`                                  | see the [Researcher reference](researcher.md)                                                                                                                                                                                                                                                                                          | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `[notifications]`                                                       | `herdr`, `fallback`, `pm_stale_minutes`, `pm_stuck_minutes`                                                                                                                                                                                                                                                                                                | At least one of `herdr` and `fallback` must be true. `pm_stale_minutes` (1 to 1440, default 10): once the PM's oldest pending message has waited this long, `cstan status` and the dashboard show a stale-mail banner. The PM wake backoff starts at `timers.pm_wake_interval_seconds`, doubles and is capped by a constant of 1800 s that is not configurable. `pm_stuck_minutes` (default 15, 0 turns it off, else 2 to 240; a restart applies it): a PM that has waited this long is stuck, which opens one episode with a `pm.stuck` event, shows on the PM card and is pushed through ntfy (`pm_stuck`). The reasons are unread action mail (the PM idle or done), no progress (the PM working with no activity, recorded wait or tool process; silent for that many minutes after a controller start and not under a paused run) and a prompt (the PM blocked or with an open prompt relay). Set it above `pm_stale_minutes` (default 10), which is the unchanged Herdr notice for pending PM mail. An episode ends after 60 seconds clear; the same PM becoming stuck again within 30 minutes of an alerting episode does not alert again (fixed, not configurable).                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[timers]`                                                              | `max_deferral_seconds`, `max_busy_deferral_seconds`, `pm_ack_timeout_seconds`, `pm_notify_after_seconds`, `notify_interval_seconds`, `stall_after_seconds`, `worker_ack_timeout_seconds`, `finding_check_seconds`, `pm_wake_after_seconds`, `pm_wake_interval_seconds`                                                                 | Defaults and ranges are in `TIMER_DEFAULTS`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[agents]`                                                              | `idle_release_minutes`, `idle_release_warn_minutes`                                                                                                                                                                                                                                                                                    | Automatic release of idle workers and the warning to the PM; see [Agents](#agents).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |

There is no daemon selector any more: Rust is the only daemon, and `[daemon]` holds only the tuning and connection keys above. A `caller_connections` or `wait_connections` above `max_connections`, or a `wait_per_caller` above `wait_connections`, is refused and names the key. An old `implementation = "rust"` still loads, with a warning; `"node"` is an error (see [docs/daemon.md](../daemon.md)).

The starter file defines these roles, in this order: `pm` (PM), `supervisor` (Supervisor), `architect`, `developer`, `designer`, `operator` and `researcher` (Developer), and `reviewer` and `tester` (Verifier), all on a `claude` host. The reviewer takes `claude-opus-5-5`; the tester takes the `[defaults.Verifier]` model. The starter enables `[architect]`, `[operator]`, `[researcher]` and `[prompt_relay]`. Developer roles deny `git push`; the reviewer and supervisor also deny file-writing tools. Each agent's system prompt is a built-in command reference for its kind (`rust/crates/config/src/prompts.rs`) followed by the role's own prompt: the `prompt` text, or the contents of its `prompt_file`. The `designer` role has a built-in prompt when it sets none; see [The designer role](#the-designer-role).

### Machine and personal settings: `capstan.local.toml`

Tracking `capstan.toml` (committing it so a team shares it) is opt-in for now. By default `cstan init` puts `capstan.toml`, `capstan.local.toml` and their `~` backups in the managed `.gitignore` block, and `cstan config check` warns when `capstan.toml` is neither ignored nor tracked. `cstan init --track-config` (or `cstan init --gitignore --track-config`) leaves `capstan.toml` out of the block so you can commit it; that choice is yours, and the warning does not appear once the file is tracked. Why it is not the default: until a trust check exists, a merged branch can change the command and permission keys of a tracked `capstan.toml` (`[verify]`, `[worktree]`, `[hosts]`, `[mcp_servers]`, `[roles]`, `[operator]`; see the table below), and those run on every machine that reads the file. Treat a tracked `capstan.toml` as code that needs the same review as code. A tracked file must not hold machine paths (an absolute `scratch_dir`, `ca_file`, `token_file`, a home directory): they break on other machines and leak your layout.

Settings that belong to one machine or one person go in an optional `capstan.local.toml` in the same folder. `cstan init` always puts `capstan.local.toml` (and `capstan.local.toml~`) in the managed block, and `cstan config check` warns when it exists but git would not ignore it.

When the file exists it is laid over `capstan.toml` before anything is checked:

- Tables merge key by key, recursively: a key only one file has is kept, and a key both have takes the local value (a table in both merges again).
- Arrays and scalars in the local file replace the tracked value whole. An array is never appended to.
- The local file is a fragment: it needs no `schema_version`, and the merged result must be a complete configuration.
- The local file is read under the same checks as `capstan.toml` (a regular file, owned by you, not writable by group or others, at most 64 KB).

The controller at start-up, `cstan config check`, `cstan config sync`, `cstan status`, the verify and review readers and the daemon's re-read of `[remote]` at every merge call all see the same merged result. `cstan config check` checks the merged result; a refusal ends with `(from capstan.toml)` or `(from capstan.local.toml)` for the file that holds the key, and a TOML syntax error names its own file. A refusal never repeats a value.

`[remote]` `merge = true`, `push = true`, `push_url` and `merge_branch` are honoured only from `capstan.local.toml`. In `capstan.toml` they are refused (any value of `push_url` or `merge_branch`), so a clone or a pulled commit can neither turn phone merge or push on nor change where an approved push goes.

`[ntfy]` `enabled = true`, `server`, `topic`, `user`, `token_env`, `token_file`, `password_env`, `password_file` and `ca_file`, and `[env] pass`, are honoured only from `capstan.local.toml` as well. In `capstan.toml` they are refused (any value), so a pulled commit cannot point notifications at another server, name a host file or environment variable as an auth secret to send there, or pass host environment values into agent panes. The display keys `events`, `title_prefix`, `timeout_seconds`, `retries` and `[ntfy.event.<name>]` stay shared: they choose which events notify and how they look, and with no local server they send nothing. `topic` is local-only because on a shared public server a topic name decides who can read the messages.

A `capstan.local.toml` that git tracks is refused as a whole, by every reader: the error names the file and says to untrack it (`git rm --cached capstan.local.toml`), and `cstan config check` reports it as an error (exit 3). Nothing from it is honoured, so a repository that force-adds the file cannot ship its own settings. The check runs `git ls-files --error-unmatch` in the project folder (no shell, cleared environment). Outside a git work tree the file is honoured. When git is missing or fails for another reason, the host fails closed: `[remote]` `merge = true`, `push = true`, `push_url` and `merge_branch` in the local file are refused until git works, and so are the local file's `[ntfy]` `enabled = true` and connection keys and `[env] pass`; its other keys stay honoured. `[ntfy]` secrets stay as `token_file`/`token_env` (or `password_file`/`password_env`); a secret value is never written in either file.

### Shared vs local keys

Keys that run commands on the host, send data off the host or widen what an agent may do, and whether a tracked `capstan.toml` (shared by a clone, once you opt in) may hold them today. "Local only" means the tracked file is refused and `capstan.local.toml` is honoured (and refused when git cannot confirm it is untracked).

| Key | Effect | In tracked `capstan.toml` |
| --- | --- | --- |
| `[remote]` `merge = true`, `push = true`, `push_url`, `merge_branch` | merges into the main branch, pushes to a remote | Local only |
| `[ntfy]` `enabled = true`, `server`, `topic`, `user`, `token_env`, `token_file`, `password_env`, `password_file`, `ca_file` | sends notifications and an auth secret to a server | Local only |
| `[env]` `pass` | host environment values reach agent panes, `setup`, `teardown` | Local only |
| `[ntfy]` `events`, `title_prefix`, `timeout_seconds`, `retries`, `event` | which events notify and how | Allowed (sends nothing without a local server) |
| `[verify]` commands | shell commands run on the host in scratch worktrees | Allowed |
| `[worktree]` `setup`, `teardown` | shell commands run in each worktree | Allowed |
| `[cache]` dirs and presets | host directories shared into worktrees and the build environment | Allowed |
| `[hosts.<name>]` `command`, `kind`, `model`, `permission_mode` | the program started for an agent and its default permission mode | Allowed |
| `[mcp_servers.<name>]` `command`, `args` | programs started for roles that list them in `mcp` | Allowed |
| `[roles.<name>]` `write_dirs` | folders a Developer role on a claude host may write although `Write` and `Edit` are denied | Allowed (widens what an agent may do; review it like `allow`) |
| `[roles.<name>]` `allow`, `deny`, `permission_mode`, `host`, `model`, `hooks`, `mcp`, `prompt`, `prompt_file` | an agent's tool permissions, host, hooks and instructions | Allowed |
| `[defaults]`, `[defaults.<Kind>]` `model`, `permission_mode` | the same, as defaults | Allowed |
| `[operator]` `auto_approve`, `auto_approve_prefix` | approvals given without asking | Allowed |
| `[researcher]` `output_dir`, `user_agent`; `[review]`, `[design]` commands | where research is written, web access identity, review and design commands | Allowed |

Everything marked Allowed is trusted project config: anyone who can land a commit on `capstan.toml` can run commands on the machines that read it.

#### Opting in to a tracked `capstan.toml`

1. Create `capstan.local.toml` and move into it the `[remote]` table (at least `merge`, `push`, `push_url`, `merge_branch`; the tracked file now refuses them), the `[ntfy]` table (its display keys may stay shared), `[env] pass` and any key that holds an absolute path of your machine (`scratch_dir`, `ca_file`, `token_file`, ...). Delete them from `capstan.toml`.
2. Run `cstan init --gitignore --track-config`: it rewrites the managed block to ignore `capstan.local.toml` and drops the `capstan.toml` lines. A `capstan.toml` line you added yourself outside the block is yours to remove.
3. Make sure git does not track `capstan.local.toml` (`git rm --cached capstan.local.toml` if it does). Run `cstan config check`, then commit `capstan.toml`.

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
| `[ledger] cache_mb`, `mmap_mb`, `backup_max_age_days` | 64, 256, 14 | 0 to 1024, 0 to 4096, 0 to 3650. `cache_mb` is memory the daemon holds; `0` switches each off. |
| `[daemon] status_cache_ms`, `driver_read_first`, `snapshot_reads_per_minute` | 2000, true, 0 | 0 to 60000 (`0` turns the status and feed caches off); `false` keeps the mutate-every-tick driver path; 0 to 600 (`0` no limit). |
| `[daemon] max_connections`, `caller_connections`, `wait_connections`, `wait_per_caller`, `repeat_window_seconds`, `busy_retry_ms` | 64, 32, 32, 4, 60, 500 | 8 to 1024; 1 to `max_connections`; 1 to `max_connections`; 1 to `wait_connections`; 0 to 600 (`0` off); 50 to 10000. |
| `[layout] min_pane_columns`, `min_pane_rows` | 60, 12 | 1 to 500, 1 to 200. |
| `[notifications] herdr`, `fallback` | true, true | Send to Herdr; also write `.capstan/state/notifications.jsonl`. They must not both be false. `pm_stale_minutes` (default 10, 1 to 1440): a message for the PM unanswered this long (pulled but not acknowledged counts) raises one "stale" notification. `pm_stuck_minutes` (default 15; 0 off, else 2 to 240): a PM that waits this long is stuck, see the `[notifications]` row above; set it above `pm_stale_minutes`. |
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
| `[agents] idle_release_minutes` | 120 | `0`, or 15 to 10080. Minutes an agent may sit idle before it is released; `0` turns automatic release off. See [Agents](#agents) and [Idle release](workflow.md#idle-release). |
| `[agents] idle_release_warn_minutes` | 10 | 1 to 60, and less than `idle_release_minutes`. Minutes before the release at which the PM is warned. |
| `[supervision] enabled`, `check_seconds` | true, 300 | The controller keeps one Supervisor running while workers are active; a Supervisor is a Claude session and uses usage. |
| `[roles.<name>] hooks` | none | `off` or `inherit`: whether the role runs your own Claude Code hooks (claude hosts only). |
| `[roles.<name>] memory` | `true` on a PM (claude host) | PM only: the PM may create and edit files in its own Claude Code memory folder, `<claude config dir>/projects/<slug>/memory/` (`MEMORY.md` included; `<slug>` is the project's real path with every character that is not a letter or digit turned into `-`; the config dir is `$CLAUDE_CONFIG_DIR` or `~/.claude`). Set `false` for the full `Write`/`Edit` deny. |
| `[roles.<name>] max_active` | unlimited | See [`max_active`](#max_active). |
| `[roles.<name>] write_dirs` | none | `write_dirs = ["docs/research"]`: Developer role on a claude host only: up to 8 folders, relative to the agent's worktree, the role may write although `Write` and `Edit` are denied. Letters, digits, `.`, `_` and `-`; no `.`, `..`, `.git` or `.capstan` segment; no duplicates. Never set by default: only a file that lists it grants it. |
| `[messages] max_body_bytes` | 4096 | `max_body_bytes = 8192`: 512 to 15360: the most bytes a message sent with `cstan send` may carry; longer text goes in a file and `cstan send --file`. |
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
| `max_body_bytes` | `4096` | integer 512 to 15360 | The most bytes the body of a `cstan send` from an agent other than the PM may have. Longer text goes in a file committed on the agent's branch and `cstan send --file`. See [Message and text limits](#message-and-text-limits). |
| `notice_class` | empty | table of notice kind to `"action"` or `"info"` | Changes the class of a system notice kind. Only the kinds marked overridable below may be changed. |

`cstan config check` refuses a value outside these (`messages.max_body_bytes must be an integer from 512 to 15360`), an unknown key, an unknown notice kind and a kind that is not overridable, with an error that names the key (for example `messages.notice_class.delivery_problem`).

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

### Message and text limits

Sizes in bytes. A text over its limit is refused with the limit in the message, or (a verdict notice) cut with a pointer to the full text.

| Text | Limit | What happens over it |
| --- | --- | --- |
| Body of a `cstan send` to an agent or `@pm`, from an agent other than the PM | `[messages] max_body_bytes`, default `4096`, 512 to 15360 | Refused: `a message body may be at most <n> bytes ([messages] max_body_bytes); write the text to a file on your branch, commit it and send cstan send --file <path> --summary <text>`. |
| The same body, from the PM | 15360 | Refused. 15360 is the terminal text limit, 16384, minus 1024 for the message frame; `max_body_bytes` cannot go above it. |
| Message to `@user` | 4096 | Refused. |
| `cstan send --file` summary | 1000 | Refused. |
| Review text (a verdict's findings) | 16000 | Refused with the length and "Keep each finding to one line that says what is wrong and where". Under the limit the text is stored whole, never cut. |
| Review text quoted in a verdict notice | 4000 whole | Over 4000 the notice shows the first 3600 bytes and `[cut: showing <n> of <total> bytes; full text: cstan inspect <review-id>]`. |
| One request or reply frame on the control socket | 65536 | Refused. |
| Event feed text | 4096 | Cut. |

A file handed over with `cstan send --file` has no size limit of its own: the message carries only its path, branch, commit, size and summary. See [`cstan send --file`](commands.md#messaging-and-workers).

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

### Control socket

The controller listens on a Unix socket. When the in-project path `.capstan/state/control.sock` fits the platform limit (107 bytes on Linux, 103 on macOS), that is the socket. Otherwise the socket is `/tmp/capstan-<uid>/<hash>.sock`, where `<hash>` is the first 32 hex characters of the sha256 of the canonical state directory path. No environment variable changes this; `CAPSTAN_SOCKET` only tells agents which socket to use.

The fallback directory `/tmp/capstan-<uid>` is created with mode `0700`. Capstan checks it on every use: if it is owned by another user, or has any group or other permission bit, it is refused and the daemon does not start. To fix it, remove the directory if it is yours to remove, or have its owner remove it or set it to `0700` (`chmod 700 /tmp/capstan-<uid>`), then start the daemon again. A leftover fallback socket from a stopped daemon is removed at start only when nothing listens on it; a socket that still answers is never removed.

The limit is checked against the canonical path (symlinks resolved; on macOS `/var` is `/private/var`, which adds 8 bytes). A project that fitted only through a non-canonical path now resolves to the fallback. A daemon an older version started on the old path is then not found after the upgrade: run `cstan stop` with the old binary, or restart the daemon, once. Projects whose canonical path fits are unaffected.


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

`merge = true`, `push = true`, `push_url` and `merge_branch` are honoured only from an untracked `capstan.local.toml` ([Machine and personal settings](#machine-and-personal-settings-capstanlocaltoml)); in the tracked `capstan.toml` they are refused, and a `capstan.local.toml` that git tracks is refused whole. `capstan.toml` is often committed, so a cloned repository can arrive with its own `[remote]` table. Review `[remote]` before running `cstan` in a repository you did not write.

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
| `remote.*.pushurl` |
| `extensions.partialClone` |
| `remote.*.promisor` |
| `remote.*.partialclonefilter` |
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

## Impact

The `[impact]` section shapes the details stored once when an integration is merged: the changed paths grouped into areas, the added migrations and the next steps a change calls for. An absent section means the defaults below. Unknown keys, values of the wrong type and an `impact` that is not a table are refused by `cstan config check`, with a message naming `impact.<key>`. A stored row reflects the section as it was at merge time and is never rewritten.

```toml
[impact]
areas = ["rust/crates/*", "docs"]
migrations = ["migrations/*.sql"]
next_steps = [{ paths = ["migrations/*.sql"], step = "back up the ledger, then restart the daemon" }]
```

- `areas` (array of patterns, at most 64, default empty): `a/b/*` maps a path under `a/b` to `a/b/<child>`; a plain prefix such as `docs` maps a path under it to itself. The longest matching pattern wins. A path that matches none belongs to the area named by its first path component, which is also the whole default. A pattern is a relative path of 1 to 200 bytes; the only wildcard is a final `/*`.
- `migrations` (array of globs, at most 64, default none): the added paths that match are listed as migrations. In a glob `*` matches any run of characters but `/`, `?` one character but `/`, and `**` any run including `/`. A glob is a relative path of 1 to 200 bytes with no empty, `.` or `..` segment, no backslash and no `[`, `]`, `{`, `}` or `!`.
- `next_steps` (array of `{ paths = [globs], step = text }`, at most 10, default none): a step is listed, in this order and once, when a changed path matches one of its `paths` globs (at least one, at most 64). `step` is 1 to 200 bytes without control characters.

## Merge and push safety

A merge or a push from the app refuses a repository whose local config sets a key below. `Scope` says which of the two refuses it: `merge`, `push` or `both` (`*` stands for any name). Every key of the merge list is also in the push list, so today no key is merge-only.

| Key | Scope |
| --- | --- |
| `merge.*.driver` | both |
| `filter.*.clean` | both |
| `filter.*.smudge` | both |
| `filter.*.process` | both |
| `diff.external` | push |
| `diff.*.textconv` | push |
| `diff.*.command` | push |
| `core.sshCommand` | push |
| `core.askPass` | push |
| `core.gitProxy` | push |
| `core.pager` | push |
| `core.editor` | push |
| `core.attributesFile` | both |
| `core.worktree` | both |
| `core.bare` | both |
| `core.alternateRefsCommand` | both |
| `sequence.editor` | push |
| `submodule.*.update` | push |
| `mergetool.*.cmd` | push |
| `push.recurseSubmodules` | push |
| `http.proxy` | push |
| `http.proxyAuthMethod` | push |
| `http.sslVerify` | push |
| `http.sslCert` | push |
| `http.sslKey` | push |
| `http.sslCAInfo` | push |
| `http.sslCAPath` | push |
| `http.sslBackend` | push |
| `http.pinnedPubkey` | push |
| `http.followRedirects` | push |
| `http.extraHeader` | push |
| `http.cookieFile` | push |
| `http.curloptResolve` | push |
| `http.*.proxy` | push |
| `http.*.proxyAuthMethod` | push |
| `http.*.sslVerify` | push |
| `http.*.sslCert` | push |
| `http.*.sslKey` | push |
| `http.*.sslCAInfo` | push |
| `http.*.sslCAPath` | push |
| `http.*.sslBackend` | push |
| `http.*.pinnedPubkey` | push |
| `http.*.followRedirects` | push |
| `http.*.extraHeader` | push |
| `http.*.cookieFile` | push |
| `http.*.curloptResolve` | push |
| `ssh.variant` | push |
| `remote.*.proxy` | push |
| `remote.*.proxyAuthMethod` | push |
| `credential.*` | push |
| `gpg.program` | push |
| `gpg.*.program` | push |
| `remote.*.receivepack` | push |
| `remote.*.uploadpack` | push |
| `remote.*.vcs` | push |
| `remote.*.pushurl` | push |
| `extensions.partialClone` | both |
| `remote.*.promisor` | both |
| `remote.*.partialclonefilter` | both |
| `url.*.insteadOf` | push |
| `url.*.pushInsteadOf` | push |
| `protocol.*.allow` | push |
| `include.path` | both |
| `includeIf.*` | both |

### Allowed in merges

These keys do not stop a merge, each for the reason given. Any key that is not in the merge list is allowed, for example `core.excludesFile`, `filter.*.required` alone, `merge.*.name`, `core.untrackedCache` and the other `extensions.*` keys.

- `core.sparseCheckout`, `core.sparseCheckoutCone`: only narrow which files are checked out.
- `core.symlinks`, `core.autocrlf`, `core.eol`: change how files are written, never what runs.
- `diff.*.textconv`, `diff.external`: run only for a diff, and a merge makes none.
- `gpg.program`, `gpg.format`: used only to sign, and a merge commit made here is not signed.
- `submodule.*.update`, `submodule.*.url`: used only by submodule commands, which a merge does not run.
- `extensions.objectFormat`: names the hash; `sha256` is refused separately (see below).

### Refused setups

- `alternates`: a repository with a non-empty `objects/info/alternates` (a clone made with `--reference` or `--shared`) is refused for merges from the app. Run `git repack -a -d`, then remove `.git/objects/info/alternates`.
- `git-lfs`: a repository that uses Git LFS is refused for merges from the app.
- `sha256`: a repository with the sha256 object format is refused for merges from the app.
- `shallow-grafts`: a shallow clone or a repository with `info/grafts` is refused for merges from the app. Run `git fetch --unshallow` and remove `.git/info/grafts`.
- `safe-directory`: a checkout owned by another uid, or one that relies on `safe.directory` in the global config, is refused.

### Residuals

What these checks do not cover.

- `global-config`: the trusted global and system git config is not checked, and variables the root calls drop are not forwarded.
- `dbus`: `USER`, `LOGNAME`, `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` are not forwarded.
- `home`: `HOME` must be set.
- `batchmode`: ssh runs in BatchMode, so an unknown host key fails the call.
- `group-kill`: killing the process group on a timeout also ends helper daemons and their cache writes.
- `sparse-checkout`: `core.sparseCheckout` is allowed, so a merge can touch files outside the sparse set.
- `isolated-cost`: the isolated merge checkout costs time and disk in a large repository.
- `identity`: a merge needs a git identity (`no_identity` otherwise).
- `gitfile-outside`: a `.git` file that points outside the project root is accepted.
- `http-alternates`: `objects/info/http-alternates` is not checked.
- `lc-all`: stderr is classified only with `LC_ALL=C`, so a differently localised message may be reported as `merge_failed`.
- `credential-helper`: a repository-local `credential.*` is refused for push; set `credential.helper` in the global config.

### Re-check

The merge list is re-checked against git's documentation whenever the merge's git command set or minimum version changes.

## Push notifications (ntfy)

The `[ntfy]` section sends a push notification to an [ntfy](https://ntfy.sh) server when something needs a person. An absent section, or `enabled = false`, sends nothing. Unknown keys, values of the wrong type and an `ntfy` that is not a table are refused by `cstan config check`, with a message naming `ntfy.<key>`; a refusal never repeats a rejected value. The syntax rules apply always; the checks that look at the host (`ca_file`, `token_file`, `password_file` must exist) run only while `enabled = true`, so a committed capstan.toml with `enabled = false` loads on any machine.

```toml
[ntfy]
enabled = true
server = "https://ntfy.example.org"
topic = "my-project"
token_env = "NTFY_TOKEN"
events = ["user_question", "agent_lost"]

[ntfy.event.agent_lost]
priority = "urgent"
tags = ["warning", "skull"]
```

- `enabled` (true or false, default false): whether notifications are sent.
- `server` (string, required when enabled): `scheme://host[:port][/path]`. Trailing slashes are trimmed. `https` always; `http` only for the host `localhost`, `127.0.0.1` or `[::1]` (the bracketed form is the only IPv6 spelling accepted). User information, a query and a fragment are refused.
- `topic` (string, required when enabled): matches `^[A-Za-z0-9_-]{1,64}$`.
- Authentication: none (anonymous), or exactly one of `token_env`, `token_file`, `user` with `password_env`, or `user` with `password_file`. An `_env` value names an environment variable (`^[A-Z_][A-Z0-9_]*$`); a `_file` value is an absolute path to a regular file owned by you with mode 0600, not a symlink, holding the secret (1 to 4096 bytes, surrounding whitespace removed). The secret is read again for every send. A key named `token`, `password` or `secret` is refused: capstan.toml must not hold secrets.
- `user` (string): matches `^[A-Za-z0-9._@-]{1,64}$`; needs a password key, and a password key needs `user`.
- `events` (array of event names, default all): which events notify.
- `timeout_seconds` (integer 1 to 60, default 10): the limit for one send.
- `retries` (integer 0 to 5, default 2): further tries after a failed send.
- `ca_file` (absolute path, default unset): a PEM bundle of certificate authorities to trust for `server`.
- `title_prefix` (string, at most 40 characters, no control characters, default unset): the start of every title. Unset means the project name from `[project]` with control characters removed and cut to 40 characters, and `Capstan` when there is none.
- `[ntfy.event.<name>]` with `priority`, `tags` and `click`, per event. A table overrides only the keys it sets.
  - `priority`: `min`, `low`, `default`, `high` or `urgent`, or an integer from 1 to 5.
  - `tags`: at most 5 strings, each matching `^[A-Za-z0-9_+-]{1,32}$`.
  - `click`: an `https://` URL opened when the notification is tapped.

The events, with the priority and tag each has when no table sets them:

| Event | Sent when | Default priority | Default tag |
| --- | --- | --- | --- |
| `user_question` | an agent asks you a question | high | `question` |
| `operator_proposal` | the operator proposes a command | high | `question` |
| `prompt_waiting` | an agent waits at a prompt | high | `question` |
| `design_waiting` | a design waits for approval | default | `art` |
| `plan_signed_off` | a plan is signed off | default | `white_check_mark` |
| `agent_blocked` | an agent reports it is blocked | high | `warning` |
| `agent_stalled` | an agent has made no progress | default | `hourglass` |
| `agent_lost` | an agent is gone | high | `warning` |
| `verify_failed` | a verify run failed | default | `x` |
| `integration_conflicted` | an integration hit a conflict | high | `warning` |
| `pm_stuck` | the PM has waited too long | high | `warning` |

`cstan` also sends a `test` notification (priority low, tag `test`); it cannot be selected in `events`.

Setup, the event table, troubleshooting and limits are in [Push notifications guide](ntfy.md).

The secret is never passed to agents: a variable named by `token_env` or `password_env` may not be listed in `[env]` `pass`, and a configuration that lists it is refused naming `[env] pass`. The `cstan config` JSON shows environment names and file paths only, never a secret.

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

`[cache.rust] target_dir` (default `false`), for any mode but `off`, also sets `CARGO_TARGET_DIR=cargo-target`: a persistent target directory that agents and `cstan verify` runs reuse (see the [build cache guide](build-cache.md)).

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
| `commands` | Required. 1 to 16 entries, run in order with `sh -c` in the scratch worktree; the run stops at the first non-zero exit. An entry is a non-empty string of at most 1000 characters (always runs), or a table `{ run = "...", paths = ["..."] }` that runs only when the diff touches a matching path; see [Path-aware steps](#path-aware-steps). |
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

### Path-aware steps

A `commands` entry written as a table runs its `run` command only when the change touches one of its `paths`. A plain string always runs, as before.

```toml
[verify]
commands = [
  { run = "scripts/check.sh rust", paths = ["rust/**", "Cargo.lock"] },
  { run = "scripts/check.sh dash", paths = ["dash/**"] },
  "scripts/check.sh node-guard",
]
```

- `run` is the command (same limits as a string entry). `paths` is 1 to 32 globs relative to the project root, with `/` separators, no leading `/`, no `..` and no empty segment. `*` and `?` match inside one path segment; a `**` segment matches any number of segments, none included; a glob without a wildcard names one exact path (`Cargo.lock` does not match `rust/Cargo.lock`; write `**/Cargo.lock` for that). Only the keys `run` and `paths` are accepted; `cstan config check` names the entry (`verify.commands[1].paths[0]`) it refuses.
- The diff is taken from the merge base of the verified commit and the HEAD checked out in the project root, to the commit, with renames listed as a delete and an add. A step is skipped only when that diff was read, is not empty, and touches none of the step's paths. If the diff cannot be read (no common ancestor) or is empty (the commit is already in HEAD), every step runs.
- The run record (`cstan verify show`, the `skippedSteps` field) lists each skipped step with its number, command and reason, and the log notes it. A run whose steps were all either run and passed or skipped is `passed`, for the same commit.
- A filter is part of the `commands_sha` key: adding, changing or removing a filter or a glob makes earlier passes stale, like changing a command. A configuration without filters keeps the `commands_sha` it had before filters existed.

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

A reviewer's verdict text (the text of `cstan review pass|findings <text>`) is stored whole up to 16000 bytes, so a long review is not shortened; over that `cstan review` is refused and the reviewer shortens it. The verdict notice that reaches the architect quotes up to 4000 bytes and otherwise the first 3600 bytes with the full text's size and `cstan inspect <review-id>` to read it. See [Message and text limits](#message-and-text-limits).

Each request is its own task: the reviewer judges the commit it names, treats earlier verdicts as context only, and waits for the next request until the controller ends the session.

## Agents

The `[agents]` section sets when an agent that has sat idle is released automatically, and how long before that the PM is warned. An absent section behaves as an empty one, with every default below, and `cstan config` never prints it. Unknown keys, values of the wrong type and out-of-range values are refused by `cstan config check`, with a message naming the key (`agents.idle_release_minutes`). The values are read when the daemon starts; a change applies on `cstan restart` after the trust check.

```toml
[agents]
idle_release_minutes = 120
idle_release_warn_minutes = 10
```

| Key | Meaning |
| --- | --- |
| `idle_release_minutes` | `0`, or 15 to 10080. Minutes an agent may sit idle before it is released. Default `120`. `0` turns automatic release off. |
| `idle_release_warn_minutes` | 1 to 60. Minutes before the release at which the PM is warned about the agent. Default `10`. Must be less than `idle_release_minutes` (not checked when release is off). |

## Write folders

A Developer role normally denies `Write` and `Edit` when it must not edit files (an architect that plans and integrates). Claude Code evaluates deny rules before allow rules, so a blanket `deny` of `Write` cannot be narrowed by an `allow` of a folder: the deny wins. `write_dirs` solves this for a claude host:

```toml
[roles.architect]
kind = "Developer"
host = "claude"
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)"]
write_dirs = ["docs/architecture", "docs/research"]
```

- The launcher drops only the bare `Write` and `Edit` rules from the role's `deny` and adds `Edit(/<folder>/**)` for each folder, resolved inside the agent's own worktree. Every other deny stays, including path denies such as `Write(src/**)`, `NotebookEdit`, `Agent` and the `git push` and `git merge` rules.
- It also installs a `PreToolUse` hook, `cstan guard-write --dirs <folder>...`, on `Write`, `Edit` and `MultiEdit`. The hook allows a call only when the real path of the file (symlinks resolved) lies strictly inside one of the folders, and refuses with exit 2 anything else, including an error: it fails closed. If the worktree is missing or a folder cannot be resolved, the role keeps the full deny and gets no folder.
- Rules: up to 8 folders, relative, letters, digits, `.`, `_` and `-`, no `.`, `..`, `.git` or `.capstan` segment, no duplicates. Only a `Developer` role on a `claude` host may have it (other hosts cannot enforce it), the operator role may not, and a role cannot have both this and the PM's memory guard. `cstan config check` refuses the rest. No role has it unless the file lists it: an existing project gets no folders from an upgrade. The starter gives the architect `docs/architecture` and `docs/research`.
- The folders are for long output (design notes, research) that the role commits on its branch and hands over with [`cstan send --file`](commands.md#messaging-and-workers) instead of pasting it into a message.

Why not deny the code folders instead: a list of denied folders is a stopgap, because any new top-level folder is writable until someone adds it to the list. An allow list of write folders keeps everything else closed.

## `CAPSTAN_ALLOW_FOREIGN_SOCKET`

Set to `1` in an agent shell to let `cstan` use a `CAPSTAN_SOCKET` that is not the socket of the project (nearest `.capstan/` ancestor) the working directory is in. Without it such a command exits 2 and sends no request. With it, one warning line is printed per command. `cstan inbox --hook` ignores it: on a foreign socket it stays silent and makes no request.

## The PM's memory folder

A PM role denies `Write`, `Edit`, `NotebookEdit`, `Agent` and `Task` unless you set `deny` yourself. With `memory` on (the
default), the launcher keeps `NotebookEdit`, `Agent` and `Task` denied and replaces the bare `Write`/`Edit` deny with a
guard, because Claude Code lets a deny override any allow and cannot express "everything except this folder":

- `cstan guard-write <memory-dir>` runs as a `PreToolUse` hook on `Write|Edit|MultiEdit`. It resolves the real path (symlinks
  followed) and refuses, with exit 2, any path that is not strictly inside the PM's memory folder. A relative path, a `..`
  component, a dangling symlink, unreadable or non-JSON input, or a hook that cannot run (the command ends in `|| exit 2`)
  is refused too.
- An `Edit(//<memory-dir>/**)` allow rule lets writes inside the folder go through without a prompt.
- The folder is derived at launch from the project root, never written in `capstan.toml`. If it cannot be derived safely
  (no `HOME`, an odd character in the path) the full deny stays and there is no guard.

The allowance is off, and the PM keeps the full `Write`/`Edit` deny, when any of these holds: `memory = false`; the project
sets `hooks = "off"` (the allowance needs a hook, so `hooks = "off"` turns it off); the project sets a `deny` that lists
`Write` or `Edit` (your own `deny` always wins, also over an explicit `memory = true`); the host is not Claude Code; or the
folder cannot be derived safely. The guard covers `Write`, `Edit` and `MultiEdit`; `NotebookEdit` stays denied. Your own
`allow` and `deny` still apply on top. `memory` is not allowed on other kinds.

Limit: Claude Code shortens a very long project path to a cut name with a hash, and that rule is not confirmed here. A
project whose slug is longer than 200 characters therefore gets no memory allowance (the full deny stays).

**Hooks.** With `hooks` unset or `"inherit"` the guard is installed and `disableAllHooks` is `false`, so hooks from your own
Claude Code user, project or plugin settings run for the PM as well; with `hooks` unset only the guard is added (no inbox
hook).

## Trust

A pulled `capstan.toml` can change what runs on your machine, so a controller loads it only when its checked keys are trusted on this machine. Every key is checked except `schema_version` and the keys only `capstan.local.toml` may set. Run `cstan trust` to read the changes and accept them; `cstan trust --check` fails when there are any. Records live per user in `~/.local/state/capstan/trust`, or in the absolute directory named by `CAPSTAN_TRUST_DIR` (CI and containers without a home). See [Trust check](trust.md) for the details and the upgrade path.

## Model overrides

`capstan.toml` stays the baseline for every model and is never written. A model override is a ledger row set while the project runs, for one plan or one requirement, optionally for one role kind (`Developer`, `Verifier` or `Supervisor`). It is not a `capstan.toml` key, so [`trust_keys.golden`](trust.md) and the trust digest are unchanged and the trust gate keeps guarding the file. Only the user, the PM or the operator may set one: any other caller gets `only the user, the PM or the operator may set a model override`.

**Precedence**, first match wins: the `--model` flag of `cstan spawn` > the task (requirement) override > the plan override > the role's `model` > the host's `model` > `[defaults.<Kind>]` > `[defaults]`. A kind-specific override beats an all-kinds override of the same scope. Set them with `cstan spawn --model`, `cstan plan open --model`, `cstan plan model` and `cstan link requirement --model` / `cstan link model` (see the [command reference](commands.md#model-overrides)). `cstan status` and `cstan plan show` print each agent's model and where it came from (`spawn`, `task`, `plan`, `role`, `host` or `defaults`).

**Fit.** A value must be 1 to 100 bytes, not start with a dash, hold no control character and not look like a credential. A model of one provider's family does not fit a host of another kind: `claude-*` and `opus`, `sonnet`, `haiku`, `fable`, `opusplan` belong to `claude` hosts, `gpt-*`, `codex-*`, `o1`, `o3`, `o4` to `codex` hosts; `omp` and `pi` hosts take any valid value, and an unclaimed id fits every host (the host reports unknown ids).

| Where | A role that cannot run the model | Every role of the kind cannot |
| ----- | -------------------------------- | ----------------------------- |
| `plan model`, `link model`, `plan open --model`, `link requirement --model` | set, with a warning naming the role, host and host kind | refused: `model_host_mismatch: <reason>` |
| `spawn --model` | refused when the named role cannot run it | the same |
| the spawn itself (an override read at start) | refused: `model_host_mismatch: <reason>; it came from <source>` | the same |

The spawn is the authority: it checks again with the configuration of that moment.

**When it applies.** An override is read when a worker starts. Running agents keep their model; the answer of `plan model` and `link model` lists them (`running`) so you can replace them. `cstan replace` keeps the predecessor's model. A warm reviewer that runs another model than the review now wants is not reused: the launcher answers `model_changed` when it is retargeted, the controller logs `review_reviewer_model_changed`, releases that reviewer and starts a new one with the model that now applies.

**Known limits.** `cstan link requirement --model` writes the link and the override in two steps. `cstan plan show` lists models for at most 9 agents.
