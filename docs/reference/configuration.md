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

`cstan init` writes this file. Keys are checked strictly: an unknown key is an error (`cstan config check` shows errors and warnings). The authoritative schema is `rust/crates/config/src/types.rs`.

| Table                                                                   | Keys                                                                                                                                                                                                                                                                   | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| top level                                                               | `schema_version` (1), `herdr_session`                                                                                                                                                                                                                                  | Herdr session name; default `default`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[project]`                                                             | `name`                                                                                                                                                                                                                                                                 | Must match the initialized project name.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `[limits]`                                                              | `max_workers`                                                                                                                                                                                                                                                          | Default 3, at most 16.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[ledger]`                                                              | `keep_migration_backups`                                                                                                                                                                                                                                               | Integer 1 to 50, default 3. Before each schema migration the controller copies the ledger to `.capstan/state/controller.sqlite.pre-v<N>-<timestamp>.sqlite`. After a successful migration only the newest this-many such backups (by schema version, then timestamp) are kept and older ones are deleted. Only files with exactly that name are deleted; a failed delete is logged to the daemon's stderr and never fails startup.                                                                                                                                                                                                                                                                                                                                                                                                 |
| `[layout]`                                                              | `spawn` (`tab` or `pane`), `split` (`auto`, `right`, `down`), `pm_width_percent`, `min_pane_columns`, `min_pane_rows`                                                                                                                                                  | Starter file sets `spawn = "pane"`, `pm_width_percent = 60`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `[worktree]`                                                            | `setup`, `setup_timeout_seconds`, `teardown`, `teardown_timeout_seconds`                                                                                                                                                                                               | Optional. `setup` is a shell command run once per new worktree with `sh -c`, the worktree as the working directory, e.g. `setup = "make deps"`, whatever your project needs to be ready in a fresh checkout. It applies to workers, replacements and the architect, not the PM. `setup_timeout_seconds` is 1 to 3600, default 600, and is refused without `setup`. A failing or timed-out setup aborts the spawn. The setup command runs with the same filtered environment as agents (the default allowlist plus `[env] pass`), not the daemon's full environment. `setup` is trusted project config, like the other commands in `capstan.toml`: anyone who can edit the file can run commands. See [Worktree teardown](#worktree-teardown) for `teardown`. |
| `[cleanup]` | `branches`, `worktrees`, `include_unlanded` | Optional. `branches` is `"auto"`, `"ask"` or `"off"` (default `"ask"`). `worktrees` is `"release"` or `"keep"` (default `"release"`). `include_unlanded` is true or false (default false). See [Cleanup](#cleanup). |
| `[supervision]`                                                         | `enabled`, `check_seconds`                                                                                                                                                                                                                                             | While workers are active the controller keeps one Supervisor running and sends it a routine check (default every 300 s). A Supervisor uses model usage; set `enabled = false` to turn it off.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `[liveness]` | `enabled`, `check_seconds`, `confirm_checks`, `start_grace_seconds` | Optional. See [Liveness](#liveness). |
| `[messages]` | `wake`, `send_default`, `info_wake_after_seconds`, `auto_ack_info`, `notice_class` | Optional. How messages are classed and when info wakes the PM. See [Messages](#messages). |
| `[defaults]`, `[defaults.PM]`, `.Supervisor`, `.Developer`, `.Verifier` | `model`, `permission_mode`                                                                                                                                                                                                                                             | Used when a role sets none. `permission_mode` is one of `default`, `acceptEdits`, `plan`, `auto`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `[cache]` | `presets`, `detect`, `root`, `scope`, `max_size_gb`, `max_age_days`, `rust`, `python`, `node`, `go`, `entries`, `setup_reuse` | Optional. Shared tool caches for agents. See [Cache](#cache). |
| `[env]`                                                                 | `pass`                                                                                                                                                                                                                                                                 | Extra environment variable names copied from where the daemon started into every agent.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `[hosts.<name>]`                                                        | `kind` (`claude`, `codex`, `omp`), `command`, `shell_command_timeout_seconds`, `wait_timeout_seconds`                                                                                                                                                                  | `codex` and `omp` hosts run unattended with full access; `cstan config check` warns about it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `[roles.<name>]`                                                        | `kind` (`PM`, `Developer`, `Verifier`, `Supervisor`), `host`, `model`, `permission_mode`, `allow`, `deny`, `hooks` (`off` or `inherit`), `prompt` or `prompt_file`, `max_active`                                                                                                     | Role names match `^[a-z][a-z0-9-]{0,31}$`. A PM may not edit files or start subagents by default, and a Supervisor may not edit or push, unless the role sets `deny` itself.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `[architect]`                                                           | `enabled`, `role`, `plan_review` (`high_risk`, `always`, `never`), `reviewer_role`, `max_packages` (1 to 20, default 8), `max_review_rounds` (integer 1 to 20, default 5; the design-round limit of a plan review), `plan_amend` (boolean, default true; allows `cstan plan amend`), `count_toward_worker_limit`, `high_risk_triggers`                                                                                             | Optional planning role; see [Planned work](workflow.md#planned-work-and-the-architect). The role must be a Developer role on a `claude` host; `reviewer_role` must be a Verifier role.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[operator]`                                                            | see the [Operator reference](operator.md)                                                                                                                                                                                                                              | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[prompt_relay]`                                                        | `enabled`, `capture_ttl_seconds` (60 to 3600, default 600)                                                                                                                                                                                                             | Optional; lets the PM show you a blocked worker's permission prompt and type the answer you pick. See [Prompt relay](workflow.md#prompt-relay).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[nexora]`                                                              | `track` (`ask`, `always`, `never`), `default_action` (`create`, `link`, `none`)                                                                                                                                                                                        | Policy only: whether the PM mirrors work into Nexora. Connection details stay in `.nexora.toml`, which Capstan never reads. See [Nexora tracking](workflow.md#nexora-tracking).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[researcher]`, `[mcp_servers.<name>]`                                  | see the [Researcher reference](researcher.md)                                                                                                                                                                                                                          | Optional; off unless `enabled = true`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `[notifications]`                                                       | `herdr`, `fallback`, `pm_stale_minutes`                                                                                                                                                                                                                                | At least one of `herdr` and `fallback` must be true. `pm_stale_minutes` (1 to 1440, default 10): once the PM's oldest pending message has waited this long, `cstan status` and the dashboard show a stale-mail banner. The PM wake backoff starts at `timers.pm_wake_interval_seconds`, doubles and is capped by a constant of 1800 s that is not configurable.                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `[timers]`                                                              | `max_deferral_seconds`, `max_busy_deferral_seconds`, `pm_ack_timeout_seconds`, `pm_notify_after_seconds`, `notify_interval_seconds`, `stall_after_seconds`, `worker_ack_timeout_seconds`, `finding_check_seconds`, `pm_wake_after_seconds`, `pm_wake_interval_seconds` | Defaults and ranges are in `TIMER_DEFAULTS`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |

There is no `[daemon]` table any more: Rust is the only daemon. An old `implementation = "rust"` still loads, with a warning; `"node"` is an error (see [docs/daemon.md](../daemon.md)).

The starter file defines these roles: `pm` (PM), `developer` and `designer` (Developer), `reviewer` and `tester` (Verifier), and `supervisor` (Supervisor), all on a `claude` host. Developer roles deny `git push`; the reviewer and supervisor also deny file-writing tools. Each agent's system prompt is a built-in command reference for its kind (`rust/crates/config/src/prompts.rs`) followed by the role's own prompt: the `prompt` text, or the contents of its `prompt_file`. The `designer` is the one starter role with a `prompt_file`; see [The designer role](#the-designer-role).

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

A notice kind that is not overridable is always an action message, so a problem that needs the PM cannot be hidden in the digest.

Upgrade note: this table is part of the configuration hash, and the ledger schema that carries message classes is 40. An older binary refuses a ledger at schema 40. To go back, stop the daemon, restore the backup the migration made (`.capstan/state/controller.sqlite.pre-v<N>-<timestamp>.sqlite`, see `[ledger]`) over `controller.sqlite`, and start the older binary.

### `prompt_file`

A role may set `prompt_file` instead of `prompt` (never both). The rules:

- The path is relative to the project root and must stay inside it.
- It must be a regular file of at most 64 KiB, valid UTF-8, with no control characters and no text that looks like a credential.
- It is read when the config loads and again when an agent launches, so an edit takes effect at the next launch.
- `cstan config check` prints the file's path and its SHA-256 hash for the role. A missing file fails the load with `prompt_file does not exist`.

### The designer role

`cstan init` writes `roles/designer.md` next to `capstan.toml`, and the starter `[roles.designer]` points at it. The file holds the designer's workflow (and the commit-subject and no-attribution rule; the same text is embedded in `rust/crates/config/src/designer.md`, which `cstan init` writes, so the two change together): Claude Design, a brief, 2 or 3 directions, the build, Playwright verification and an anti-slop standard. Edit the file to change the workflow; `cstan init` never overwrites an existing `roles/designer.md`, and it writes none when `capstan.toml` already exists.

The starter role sets:

```toml
[roles.designer]
mcp = ["playwright"]
allow = ["Bash(git *)", "Skill", "Artifact", "DesignSync", "mcp__playwright", "Bash(python3 -m http.server *)"]
prompt_file = "roles/designer.md"
```

`[mcp_servers.playwright]` (pinned `@playwright/mcp`, headless and isolated) is defined in the starter file and shared with the optional researcher. `Artifact` and `DesignSync` exist only when Claude Code on the machine is signed in to claude.ai; without them the prompt tells the designer to say so and report the work as unverified. `DesignSync` is allowed in full, so only the prompt keeps it read-only. Because `mcp` makes Claude Code run with `--strict-mcp-config`, the designer sees only the servers it lists, not your other MCP servers.

To switch Playwright off for the designer, remove the `mcp` line and the `mcp__playwright` allow rule. The prompt then reports the screenshot and contrast checks as unverified. To adopt the workflow in an existing `capstan.toml`, add those three lines to `[roles.designer]`, remove its `prompt`, define `[mcp_servers.playwright]`, and copy `roles/designer.md` from a fresh `cstan init`.

`[researcher]` (`enabled`, `role`, `output_dir`, `user_agent`) turns on the web-research role; `[mcp_servers.<name>]` (`command`, optional `args`) defines an MCP server and `roles.<name>.mcp` lists the servers a `claude`-host role may use. See the [Researcher reference](researcher.md).

### Running a worker on Codex or OMP

A Developer or Verifier role can run on a Codex or OMP host instead of Claude Code. Checked against Codex 0.160.0 and OMP 18.3.1.

1. Declare the host (uncomment the examples in the starter file): `[hosts.codex]` with `kind = "codex"`, or `[hosts.omp]` with `kind = "omp"`.
2. Point a role at it. The role must set its own `model` (the `[defaults.<kind>]` model is a Claude model, so a Codex or OMP role that inherits it gets a model name that host does not offer), a `permission_mode` of `acceptEdits` or `auto`, and no `allow` or `deny`:

   ```toml
   [roles.codex-developer]
   kind = "Developer"
   host = "codex"
   model = "<a model name your codex CLI accepts>"
   permission_mode = "acceptEdits"
   prompt = "..."
   ```

3. Run `cstan config check`. It prints a warning for each unsandboxed role.

**Allowed:** `Developer` and `Verifier` roles without `allow`/`deny`, with `permission_mode` `acceptEdits` or `auto`.
**Refused by the configuration loader:** `PM` and `Supervisor` roles, any `allow` or `deny` (they are Claude Code tool rules), and other permission modes. The Architect and Operator roles must be on a `claude` host, so they stay on Claude.

**Commit and branch rules apply on every host.** The report check refuses commits that break the Conventional Commits or no-attribution rules whatever host made them (see [Branches, commits and releases](workflow.md#branches-commits-and-releases)). Claude hosts are started with Claude Code's attribution settings off. Codex 0.160.0 and OMP 18.3.1 appear to add no commit trailers (found by a binary string search, not a run), so the report check is the safety net.

**These workers are unsandboxed.** Codex runs with `--sandbox danger-full-access --ask-for-approval never` (its sandbox blocks the daemon socket) and OMP with `--approval-mode yolo`. Nothing blocks a `git push` or an edit outside the worktree; the role prompt asks, the controller does not enforce. Capstan trusts the worktree for Codex with a run-time override, so `~/.codex/config.toml` is not changed.

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
| `[cache.<preset>]` | Settings of a preset: `max_size_gb` for any, and `mode` for `rust`. The table is allowed only for a listed preset (or when `detect` is true). |
| `[cache.entries.<name>]` | Your own entry; see below. |
| `[cache.setup_reuse]` | `key_files` and `paths`, both non-empty lists of relative paths without `..`: the files that identify a worktree's setup result and the directories to reuse when they are unchanged. |

An entry's `max_size_gb` may not exceed the total, and the entries' sizes may not add up to more than the total.

### Presets

Each `dirs` pair is `ENV = subdirectory` of the entry's cache directory; `set` pairs are fixed values.

| Preset | Variables | Needs | Also |
| --- | --- | --- | --- |
| `rust`, mode `sccache` (default) | `SCCACHE_DIR=sccache`; `RUSTC_WRAPPER=sccache`, `SCCACHE_CACHE_SIZE` from the cap (the preset's `max_size_gb`, or the total) | `sccache` on PATH | stops the server with `sccache --stop-server` before the cache is cleared |
| `rust`, mode `shared-build-dir` | `CARGO_BUILD_BUILD_DIR=cargo-build` | `cargo` | |
| `rust`, mode `off` | none | | |
| `python` | `PIP_CACHE_DIR=pip`, `UV_CACHE_DIR=uv`, `POETRY_CACHE_DIR=poetry` | | prune: `uv cache prune` |
| `node` | `NPM_CONFIG_CACHE=npm`, `NPM_CONFIG_STORE_DIR=pnpm-store`, `YARN_CACHE_FOLDER=yarn` | | prune: `npm cache verify`, `pnpm store prune` |
| `go` | `GOCACHE=go-build`, `GOMODCACHE=go-mod` | | |

`[cache.rust] mode` accepts `sccache`, `shared-build-dir` and `off`; anything else is refused.

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

The `[design]` section controls the user's approval of a design before the packages that build on it are assigned. An absent section behaves as `require_approval = true`; `cstan config` prints a `design` object `{ configured, requireApproval }` only when the section is written. A non-boolean value or an unknown key is refused by `cstan config check`, with a message naming the key (`design.require_approval`).

| Key                | Default | Meaning |
| ------------------ | ------- | ------- |
| `require_approval` | `true`  | When true, `cstan plan assign` refuses a package that depends on a design package (`"design": true` in the plan) until the PM has recorded the user's approval of that design package's latest report with `cstan design approve`. When false, assignment ignores design approvals. Plans without design packages are not affected either way. |

```toml
[design]
require_approval = false
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
