# plan-181 ws-spawn: manual check of a real Claude Code in a change folder

Run on 2026-11-10 against a throwaway Herdr session (`capstan-test-*`) on a scratch HOME, with the real `claude` 2.1.322 (a
private copy of the sign-in) and the real `cstan daemon`. Nothing of this project's live state was used.

## What was launched

A scratch project (a git repository, so the daemon boots; the unfinished workspace mode was switched on with
`CAPSTAN_WORKSPACE_PREVIEW=1`), two sibling repositories `core` and `web` (each with a commit on `main`, a `CLAUDE.md` and
its own `.claude/settings.json` whose PostToolUse hook touches `marker-<repo>` when it is loaded), and this `capstan.toml`
(the harness also appends `[env] pass`, moved to `capstan.local.toml` because the tracked file may not carry it):

```toml
schema_version = 1
herdr_session = "{session}"

[notifications]
herdr = false
fallback = true

[timers]
pm_wake_after_seconds = 3600
pm_wake_interval_seconds = 3600

[limits]
max_workers = 1

[nexora]
track = "never"

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
allow = ["Bash(git *)", "Write", "Edit"]
hooks = "inherit"
write_dirs = ["docs"]

[repos.core]
path = "../core"
main_branch = "main"

[repos.web]
path = "../web"
main_branch = "main"

[workspace]
default_repo = "core"
worktree_dir = "../changes"
```

Commands (the harness is the throwaway test `crates/launcher/tests/ws_manual_check.rs`, not committed; its steps):

1. `cstan daemon` in the scratch project (env: scratch HOME, `CLAUDE_CONFIG_DIR`, `CAPSTAN_WORKSPACE_PREVIEW=1`).
2. launch the PM over the socket, answer the PM's trust dialog by hand (the controller never answers it).
3. `cstan spawn developer` (the worker): the launcher made `changes/developer-1/core` and `changes/developer-1/web` on
   `chore/developer-1-developer`, opened a hub tab at `changes/developer-1` and started `claude` with
   `--add-dir .../core --add-dir .../web`.
4. `cstan send developer-1 "In order: 1) git -C <folder>/core status -sb 2) git -C <folder>/web status -sb 3) cstan inbox
   4) Write <folder>/core/x.txt with x 5) Write <folder>/web/docs/y.txt with y. Then one line FINISHED: which write was refused."`

One temporary, uncommitted line chose the two repos for the spawn from an environment variable, because no CLI path names
repos before plan-174 (a package's `repos`) passes `repo_bases`; everything from `add_repo_worktree` on is the committed
code. The line and the harness were removed afterwards.

## The five lines

- [x] trust dialog answered without operator input: `cstan spawn` answered `state: started`; the daemon log holds the
  controller's own keys for the worker pane `w1:p3`: `trust_dialog_key down` then `trust_dialog_key enter`. Nobody typed in
  the pane. (This needed a fix: see below.)
- [x] role settings and hooks active, the hook that fired: the role's `allow = ["Bash(git *)"]` ran both `git status`
  calls with no prompt; the `PreToolUse:Write` hook `cstan guard-write --dirs '<core>/docs' '<web>/docs'` fired and refused
  `Write(core/x.txt)` ("guard-write: this role may write only under .../core/docs, .../web/docs"), while
  `Write(web/docs/y.txt)` was allowed. The `--settings` also carried the PostToolUse `cstan inbox --hook` (silent with
  nothing waiting).
- [x] git status ran in each worktree: the worker ran `git -C <folder>/core status -sb` and `git -C <folder>/web status -sb`
  and reported both on branch `chore/developer-1-developer`, nothing changed.
- [x] cstan inbox worked from the folder: the worker ran `cstan inbox` with the change folder as its directory ("Inbox: no
  messages") and acknowledged the operator message with `cstan ack`.
- [x] no repo's own .claude settings file loaded: both `marker-core` and `marker-web` are absent although Bash and Write
  tools ran; the repos' `.claude/settings.json` PostToolUse hooks never fired. (`web/CLAUDE.md` was loaded: "Loaded
  web/CLAUDE.md". No positive control for the settings hook was run.)

## A finding the check made

The adapter refused the trust dialog of a pane made by `create_tab`: `step trust: only a worktree pane the adapter created
has a dialog it may answer`, so every workspace spawn stopped at the dialog. The fix (committed): `create_tab` for a worker
records the tab's directory (the change folder, which is the directory the dialog names) as the pane's `worktree_path`; a PM
tab records none, so a PM's dialog is still never answered. `crates/herdr/tests/worker_tab_dialog.rs` covers it.

## Not checked

- Idle release and PM tabs in a real session; `release` was covered with the stub host only.
- `[env] pass` naming `CAPSTAN_WORKSPACE_PREVIEW`: the existing check refuses any `CAPSTAN_` name in `pass`, so a pane never
  gets it; the switch is also removed from the environment after `pass` is applied (tested with it set in the base
  environment).

## Rerun of line 5 with a positive control (2026-11-10)

Line 5 above showed a missing marker without showing that the marker can appear. Rerun in a fresh scratch directory (no
live project state): a git repository `core` whose `.claude/settings.json` has a PostToolUse hook (`echo fired >> marker-core`),
and a worktree of it at `changes/dev-1/core`, a folder that is not a repository.

1. Positive control: `claude -p "Run git status"` started inside `core`, a normal start. The Bash tool ran and
   `marker-core` appeared (`fired`). So the hook fires when claude starts in the repo.
2. The workspace shape: `claude -p` started in `changes/dev-1` (not a repository) with `--add-dir changes/dev-1/core`, as
   the launcher starts a worker; the agent ran `git -C .../core status -sb` and wrote `core/x.txt` (both tools ran, the file
   exists). `marker-core` was absent: the worktree's `.claude/settings.json` was not loaded.

Limit: this reproduces the folder and `--add-dir` shape with `claude -p`, not the daemon-started interactive worker of the
first check (that harness was not kept, and this account's credentials were not copied into a scratch home). The first
check's workspace run saw the same absence for both repos.

## Teardown through `loops/mod.rs`

The plan's item about teardown in `loops/mod.rs` turned out to be moot: teardown of a workspace worker runs through
`release_workspace` on the agent's ledger rows (one worktree at a time), so the loop needs no change.
