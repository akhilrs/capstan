# Launcher parity sequences

Each `*.json` file here is one group: `{ "sequences": [ ... ] }`. The exporter (`node dist/test/launcher-parity-export.js`)
runs every sequence of every file (found by glob) against the real Node `Launcher` and writes the steps and what happened to
`rust/crates/launcher/tests/parity/<group>.json`. The Rust replay (`rust/crates/launcher/tests/parity.rs`) runs the same
steps on `capstan-launcher` and compares. A new group file needs no change in the exporter, the staleness test
(`test/launcher-parity.test.ts`) or the replay.

## The world of a sequence

A sequence runs in its own scratch directory: a git repository with one commit (fixed author, committer and dates, so the
commit ids are the same in every run) as the project, the controller's state directory under it, a directory for the
worktrees the stub adapter names and one for the prompt files. The controller is a real `ControllerCore` with a clock that
starts at 2026-01-01T00:00:00.000Z and ticks 1 ms per reading and a randomness stream seeded by `seed` (default: the name;
`test/kernel-parity-hooks.ts`, `SeededEnv` on the Rust side). The launcher reads those through the ledger and nothing else,
so both implementations draw in the same order. Herdr is the stub of `test/launcher-stubs.ts` (the Rust form is
`tests/common/stub.rs`) behind a recorder; the stub makes the worktree on disk with `git worktree add`, as Herdr does. A
`git` shim first on PATH logs the arguments of every git command the launcher runs. Setup and teardown commands are stubs
that answer `ok` unless a step queues another outcome.

`config` (all optional) is the configuration of the roles: `maxWorkers`, `layout`, `pass`, `hostOf` (role to `codex` or
`omp`), `architect`, `operator`, `researcher`, `worktree`, `promptRelay`, `base` (variables added to the daemon's
environment) and `synced: false` (no role is synced into the ledger). The roles are `pm`, `developer`, `developer2` and
`supervisor`, plus `architect`, `operator` and `researcher` when their settings are given.

## Steps

```json
{
  "op": "spawn",
  "role": "developer",
  "options": { "title": "Fix it" },
  "as": "dev"
}
```

`"$name.path"` anywhere in a step is replaced by that part of an earlier step's result (`as` names it). The ops:

- the launcher's own: `launch_pm`, `restart_pm`, `spawn` (`role`, `options`), `release`, `replace`, `adopt_all`,
  `observe` (`agent`, `lines`), `rename_branch_for_task` (`agent`, `task`), `interrupt`, `status`, `operator_environment`,
  `in_flight`; `reopen` makes a new launcher over the same ledger and stub, as a daemon restart does;
- the world's: `git` (`cwd`, `args`: real git, not through the shim), `write` (`dir`, `file`, `text`), `report` (`agent`,
  `commit`, `summary`: an accepted report with the agent's own token), `send` (`from`, `to`, `body`: a message with the
  sender's token);
- `stub` with `do`: `fail` (`method`, `times`, `error`: `{kind, code, message}`, kinds `herdr`, `pane_gone`,
  `agent_pane_mismatch`, `pane_lost`, `phase`, `prompt_unrecognized`, `shell_not_ready`; the call is recorded and then
  fails before the stub is reached), `identity` (`pane`, `terminalId`, `agent`, `project`: what Herdr says the pane is),
  `gone` (`pane`), `forget_registry` (the adapter has lost its panes, as a restarted daemon's has), `observation`,
  `start_status`, `dialog_handled`, `screen`, `working`, `zoomed`, `layout_size` (before the PM starts), `strays`
  (`directory`, `panes`; `<worktrees>` is the worktree directory), `setup` and `teardown` (`outcomes`: `ok`, `failed` with
  `exitCode` and `output`, or `timeout`).

A step is recorded with its `result` or its `error` (`{code, message, name}`), the `calls` the launcher made to the adapter
(method and arguments; a string over 1500 characters, such as a prompt, is its SHA-256 and byte length, so prompt texts
are compared byte for byte), the `git` commands it ran, the log `events`, the `sleeps` between start attempts, the setup
and teardown `commands`, the `state` of git (worktrees and refs) and the `tablesDiff` of the ledger since the step before.
The scratch directory is written `<tmp>`. A request hash of the ledger is left out of the diff because it covers the
arguments of a mutation, which name the scratch directory.

## What a sequence cannot do

It cannot run two operations at once (the queue is tested in `tests/operations.rs`) or let time pass. The pane-reuse rule
is both in the sequences (`*reused*`, and the sequences with a terminal id or tokens that do not match) and asserted in
`tests/operations.rs`: no `closePane` call in any of them.
