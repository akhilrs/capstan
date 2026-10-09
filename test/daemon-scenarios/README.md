# Daemon scenarios and wire transcripts

Each `*.json` file here is one group: `{ "scenarios": [ ... ] }`. The exporter (`node dist/test/daemon-transcript-export.js`)
runs every scenario of every file (found by glob) against the **real Node daemon** and writes what happened to
`rust/crates/daemon/tests/transcripts/<group>.json`. The Rust replay (`rust/crates/daemon/tests/replay.rs`) runs the same
requests through `capstan_daemon::handlers` and compares. A new group file needs no change in the exporter, the staleness
test (`test/daemon-transcript.test.ts`) or the replay. Only the core package edits the exporter and the replay.

**Regeneration rule** (as in plan-25): regenerate with the exporter only. After a rebase commit only your own groups; if the
export changes another group's file, stop and tell the PM.

## How a scenario runs

For each scenario the exporter makes a scratch project in the temp directory (`.capstan/project.json`, `operator.key`, `state/`,
with an id and credential derived from the group, scenario name and seed), then:

1. `repo` (optional) builds a git repository in the project directory with fixed authors and dates, so commits have the same
   ids in every run and in the Rust replay (the replay checks them against the recorded `repo.heads`);
2. the ledger is migrated (migration timestamps use the system clock in both implementations);
3. `setup` (optional) applies kernel-sequences steps (`test/kernel-sequences/README.md`) to the ledger before the daemon opens
   it: a controller opened on the daemon's project with the seed `<seed>/setup`, closed after the last step;
4. `node --import dist/test/daemon-transcript-hooks.js dist/src/cli.js daemon` starts with `CAPSTAN_LAUNCH=off` and the seed
   `<seed>/daemon`. The hooks (`test/daemon-transcript-hooks.ts`, built on `test/kernel-parity-hooks.ts`, no `src/` edit) make
   the clock and randomness the seeded streams of the kernel parity export and keep the periodic timers from firing;
5. the requests are sent in order; the daemon is stopped with SIGTERM.

`seed` defaults to the group name, so the bootstrap rows of every scenario of a group are the same and the table diffs stay small.

## Scenario

```json
{
  "name": "authentication",
  "seed": "defaults to the group",
  "mode": "socket",
  "setup": [
    {
      "op": "createSeat",
      "context": { "credential": "$owner.credential" },
      "args": [{}],
      "as": "x"
    }
  ],
  "repo": {
    "files": { "a.txt": "a" },
    "branches": {
      "b": { "from": "main", "files": { "a.txt": "b" }, "message": "feat: b" }
    }
  },
  "requires": ["messages"],
  "requests": []
}
```

`mode: "socket"` replays the whole scenario against the real `cstan-daemon` over its socket instead of in process.
`requires` lists other groups whose routes the scenario reaches (informational).

## Requests

```json
{ "as": "operator", "command": "ping", "args": [], "label": "text", "concurrent": false, "hold_ms": 0, "dump": false }
{ "as": "none", "frame": "{not json" }
{ "as": "operator", "frame": { "json": { "v": 1, "credential": "$credential", "command": "ping", "args": [] } } }
{ "frame": { "base64": "/w==" } }
{ "frame": { "repeat": { "prefix": "...", "text": "a", "times": 65000, "suffix": "..." } }, "newline": false, "hold_ms": 6000 }
```

- `as`: `operator`, `agent:<agent-id>` (the credential a setup step created for the agent's actor: a binding with `credential` and
  `actorId`, then `registerAgent`'s result), `token:<literal>` or `none`. `command`/`args` is the standard frame
  `{v, credential, command, args}`; `frame` gives raw bytes: a string (UTF-8), `{json}` (`$credential`, `$sha:<branch>` and a whole
  string `$name.path` of a setup binding are substituted), `{base64}` or `{repeat}` (for frames that are too long to keep).
- Requests are **steps, each answered before the next**. `concurrent: true` sends the request right after the daemon _started_
  the previous one (a barrier on a `request_start` line the hooks write when `identify` begins), without waiting for its answer.
  A request that is not concurrent waits for every earlier one to be answered and logged.
- Raw bytes, `newline: false` (a partial frame), `hold_ms` (keep the connection open that long and record only the outcome:
  was it closed by the daemon, what was answered; never a duration) and frames above 65536 bytes make a request a
  **`socket`-layer** step. Everything else is a **`dispatch`-layer** step.
- `dump: true` records a table diff after the request.
- `srv` and `cmds` scenarios set state up with `setup`, not with each other's handlers.

## Transcript

Format 2 as the kernel's (`test/kernel-parity-export.ts`): `baseline` (the full ledger the group's smallest scenario ends with),
`dict` (repeated long strings and table rows, `{"$d": index}`), one scenario per line. A scenario has `name`, `seed`, `project`,
`setup` (resolved steps with their results), `repo` (with `heads`), `steps` (the request: `as`, `frame` as text or its generator,
`layer`, `newline`, optional `label`, `command`, `concurrent`, `hold_ms`; then `response` (the line, `null` when nothing was
answered), `closed`, `finished` (completion order), optional `tablesDiff`), `log` (the daemon's log entries, one JSON line each),
`exit` and `tablesDiff` (the final ledger against the baseline). The scratch path is redacted to `<project>` and `"pid":N` to
`"pid":0`.

## Replay

- A `dispatch` step goes through `handlers::handle_frame` (parse, authenticate, classify, dispatch, log): the response is
  compared as JSON (the order of members is not compared), the log entry without `ts` and `ms`, and the table diffs. Every
  request makes the same clock and randomness readings in both implementations (one at the start, one for `ms`, one for the log
  line's `ts`), so rows written later keep the same timestamps and ids.
- A step that reaches a route whose handler is still a stub (`shared::unported`) is **pending**; a `socket` step is run against
  a real `cstan-daemon` in a fresh scratch project (so it must not depend on what earlier steps did) and is pending while the
  server is a stub. `CAPSTAN_DAEMON_PARITY_STRICT=1` fails on any pending step.
- All non-Rust files under `rust/crates/{daemon,herdr,launcher,operator}/tests/` stay under 6 MiB (`tests/budget.rs`).

## Interface coverage

The methods of the Node services the daemon uses, and where the Rust interface has them (`rust/crates/<crate>/src/api.rs`).
`crates/daemon/src/deps.rs` checks every line against the sources.

```api-coverage
herdr HerdrRunner run -> run
herdr PaneRegistry paneForAgent -> pane_for_agent
herdr PaneRegistry paneEntry -> pane_entry
herdr PaneRegistry agentObservation -> agent_observation
herdr DriverAdapter guardedSend -> guarded_send
herdr DriverAdapter wakePm -> wake_pm
herdr DriverAdapter clearAfterDeferral -> clear_after_deferral
herdr LauncherAdapter createWorkspace -> create_workspace
herdr LauncherAdapter createWorktree -> create_worktree
herdr LauncherAdapter createTab -> create_tab
herdr LauncherAdapter paneLayout -> pane_layout
herdr LauncherAdapter placePane -> place_pane
herdr LauncherAdapter panesAtPath -> panes_at_path
herdr LauncherAdapter prepareShell -> prepare_shell
herdr LauncherAdapter startAgent -> start_agent
herdr LauncherAdapter answerTrustDialog -> answer_trust_dialog
herdr LauncherAdapter closePane -> close_pane
herdr LauncherAdapter paneIdentity -> pane_identity
herdr LauncherAdapter adoptPane -> adopt_pane
herdr LauncherAdapter adoptShellPane -> adopt_shell_pane
herdr LauncherAdapter reportMetadata -> report_metadata
herdr LauncherAdapter renameWorkspace -> rename_workspace
herdr LauncherAdapter renameTab -> rename_tab
herdr LauncherAdapter forgetPane -> forget_pane
herdr LauncherAdapter runInPane -> run_in_pane
herdr LauncherAdapter writePromptFile -> write_prompt_file
herdr LauncherAdapter readScreen -> read_screen
herdr LauncherAdapter capturePrompt -> capture_prompt
herdr LauncherAdapter answerPrompt -> answer_prompt
herdr LauncherAdapter interruptWorking -> interrupt_working
herdr NotifierAdapter notify -> notify
herdr AdapterAdmin version -> version
herdr AdapterAdmin close -> close
herdr AdapterAdmin paneState -> pane_state
herdr AdapterAdmin agentState -> agent_state
herdr AdapterAdmin readInput -> read_input
herdr AdapterAdmin removeWorktree -> remove_worktree
herdr ProcessActivityProbe sample -> sample
launcher LauncherService launchPm -> launch_pm
launcher LauncherService restartPm -> restart_pm
launcher LauncherService spawn -> spawn
launcher LauncherService renameBranchForTask -> rename_branch_for_task
launcher LauncherService release -> release
launcher LauncherService replace -> replace
launcher LauncherService observe -> observe
launcher LauncherService capturePrompt -> capture_prompt
launcher LauncherService answerPrompt -> answer_prompt
launcher LauncherService interrupt -> interrupt
launcher LauncherService status -> status
launcher LauncherService operatorEnvironment -> operator_environment
launcher LauncherService inFlightOperations -> in_flight_operations
launcher LauncherService adoptAll -> adopt_all
launcher LauncherService teardown -> teardown
operator OperatorService propose -> propose
operator OperatorService decide -> decide
operator OperatorService cancel -> cancel
operator OperatorService show -> show
operator OperatorService list -> list
operator OperatorService grants -> grants
operator OperatorService listGrants -> list_grants
operator OperatorService revokeGrant -> revoke_grant
operator OperatorService fullAutoOn -> full_auto_on
operator OperatorService fullAutoOff -> full_auto_off
operator OperatorService fullAutoStatus -> full_auto_status
operator OperatorService drain -> drain
operator OperatorService recover -> recover
operator OperatorService tick -> tick
operator OperatorService start -> start
operator OperatorService stop -> stop
operator RestartCoordinator preflight -> preflight
operator RestartCoordinator run -> run
```
