# Test map: the launcher, Herdr, operator, prompt-relay and ledger suites (plan-29 `suites-live`)

Where each of these Node suites went when the Rust binaries became the thing under test. Coverage by replay is by behaviour area and sequence name: a sequence covers the area, not each assertion of the Node test one
for one. Each `test/*.test.ts` file below is
**ported** (Rust test names), **covered by an existing replay** (the corpus and the sequence names), or **retired as
Node-internal** (the reason). A row that is none of the three says **not ported** and is repeated in the list at the end.

How to read a name:

- `crate::file::test` is `rust/crates/<crate>/tests/<file>.rs`; a name without a crate is in the crate the section is about.
- A **replay** is a test that runs a frozen corpus exported from Node once (nothing here runs Node): launcher sequences
  (`launcher/tests/parity.rs`, one test, `every_sequence_replays_with_the_node_launcher_outcome`, over
  `launcher/tests/parity/{pm,adopt,spawn,release,replace,teardown}.json`), the Herdr adapter replay
  (`herdr/tests/adapter_replay.rs`, `herdr/tests/parity.rs`), the kernel replay (`kernel/tests/replay.rs` over
  `kernel/tests/parity/*.json`), the daemon replays (`daemon/tests/replay.rs`, `cmds.rs`, `loops.rs`) and the operator
  replays (`operator/tests/parity.rs`). Sequence names are the ones in `test/launcher-sequences/*.json` and
  `test/kernel-sequences/*.json`.
- A **live** test drives `cstan daemon` against a real Herdr server in a throwaway `capstan-test-*` session. They are all
  `#[ignore]` and run only with `CSTAN_LIVE=1` (see "Running the live suites").

Nothing in the ported tests starts Node, and `cargo test --locked` needs no `node` on `PATH`: the daemon is `cstan daemon`,
the commands are the ones the Rust front end serves itself, and the three operator commands that go straight over the
socket (`launch`, `pm-restart`, `shutdown`) are sent by the harness. The one place the front end still hands a command to
Node, the hub's `cstan status --watch`, is a stand-in script in the harness (see "Deferred").

## Running the live suites

One at a time, in a throwaway session, after `cargo build -p cstan-front -p capstan-daemon` (the build slot rules of the
machine apply: one build at a time, `CARGO_TARGET_DIR` outside the checkout):

```
CSTAN_LIVE=1 cargo test --locked -p capstan-daemon   --test live            -- --ignored --test-threads=1
CSTAN_LIVE=1 cargo test --locked -p capstan-launcher --test live            -- --ignored --test-threads=1
CSTAN_LIVE=1 cargo test --locked -p capstan-herdr    --test live_agent      -- --ignored --test-threads=1
CSTAN_LIVE=1 cargo test --locked -p capstan-herdr    --test live            -- --ignored --test-threads=1
CSTAN_LIVE=1 cargo test --locked -p capstan-launcher --test prompt_relay_live -- --ignored --test-threads=1   # real claude
CSTAN_LIVE=1 CAPSTAN_LIVE_RESEARCHER=1 cargo test --locked -p capstan-launcher --test researcher_live -- --ignored   # real claude, npx, jq, network
```

The harness is `rust/crates/herdr/tests/common/live_env.rs` (the other crates include it with `#[path]`):

- Without `CSTAN_LIVE=1`, or without herdr, python3, git or a built `cstan` (`CSTAN_BIN` names one), a test prints
  `SKIPPED LOUDLY: ...` and passes.
- The Herdr server runs on a scratch `HOME` under `/tmp/capstan-suites-live-*` in a `capstan-test-*` session; every Herdr
  command names that session; `HERDR_*`, `XDG_*`, `CAPSTAN_*` and `CSTAN_*` variables are not passed on. The operator's
  own sessions are never listed, read or touched.
- `Live::close` stops the daemon (the `shutdown` command and a wait on its pid), stops the Herdr server, deletes the
  session, removes the directory and asserts that the session is gone from `herdr session list` and the directory is gone.
  A test that fails dumps the daemon log tail and any restart result first.
- Waiting is on state: pane list, screens, files, the daemon's answers, with a deadline only a failing run reaches. Every
  child command has a hard deadline (`output_within`), so a hung command fails the test instead of holding the run.
- The real-`claude` suites link the operator's `~/.claude` and `~/.claude.json` into the scratch `HOME` (never copied), as
  `test/researcher-live.test.ts` did, so Claude Code's own state for the scratch paths lands in the operator's file.

Last run by hand (Herdr 0.9.3, debug build, one suite at a time; each left no session, pane or `/tmp` directory behind):

| Suite | Result |
| --- | --- |
| `capstan-daemon --test live` (2 tests) | pass, 45 s |
| `capstan-launcher --test live` (4 tests) | pass, 55 s |
| `capstan-herdr --test live_agent` | pass, 16 s |
| `capstan-herdr --test live` (existing) | pass, 2 s |
| `capstan-launcher --test prompt_relay_live` (real Claude Code) | pass, 29 s |
| `capstan-launcher --test researcher_live` (real Claude Code, npx, network) | pass, 48 s |

## Summary

| Node file | Disposition |
| --- | --- |
| `launcher-lifecycle.test.ts` | covered by launcher replays + ported live; 9 areas not ported (below) |
| `launcher-spawn.test.ts` | covered by launcher replays; 4 areas not ported |
| `launcher-release.test.ts` | covered by launcher replays; 3 areas not ported |
| `launcher-wrapper.test.ts` | ported (`launcher::wrapper`), front-end selection retired |
| `launcher-git.test.ts` | ported (`launcher::git`) + replays |
| `launcher-live.test.ts` | ported (`launcher::live`, `daemon::live`) |
| `herdr-live.test.ts` | ported (`herdr::live_agent`, `herdr::live`) |
| `herdr-runner.test.ts` | covered (`herdr::runner`, `herdr::parity`) |
| `panes.test.ts` | covered by kernel replays; summary-budget tests not ported |
| `process-activity.test.ts` | covered (`herdr::parity`) |
| `prompt-relay-live.test.ts` | ported (`launcher::prompt_relay_live`) |
| `prompt-relay-commands.test.ts` | covered by daemon `cmds.rs` + kernel replays, ported refusals (`launcher::live`); some cases not ported |
| `prompt-relay-ledger.test.ts` | covered by kernel replays (`captured`) |
| `researcher-live.test.ts` | ported (`launcher::researcher_live`), opt-in |
| `operator-restart-helper.test.ts`, `operator-restart-sea.test.ts` | ported (`operator::restart`), live (`daemon::live`) |
| `operator-runner.test.ts` | ported (`operator::runner`) |
| `ledger-compat.test.ts` | ported (`ledger::compat`) |
| `migration-backups.test.ts` | ported (`ledger::compat`, `ledger::migrate`) |
| `ledger-rust-interop.test.ts` | ported (`ledger::compat`) + existing; Node-process halves retired |
| `sqlite-adapter.test.ts` | covered (`ledger::database`); Node adapter retired |

(`operator-restart.test.ts` and `operator-grants.test.ts` etc. belong to other packages; the operator-restart *binary helper*
tests are the two files above.)

## Launcher

### `launcher-lifecycle.test.ts` (43 tests)

Covered by the launcher sequence replay; the live tests drive the same operations through the daemon.

| Behaviour (Node test) | Disposition |
| --- | --- |
| PM launch: token, prompt, wrapper, pane row, watch pane | replay `launch-pm`; live `launcher::live::the_launcher_starts_a_pm_and_a_worker_delivers_readopts_after_a_restart_and_keeps_the_worker_through_a_pm_restart` (hub `opened`, watch pane, token on the pane) |
| the `cstan` wrapper is private and atomic | `launcher::operations::the_cstan_wrapper_is_private_and_runs_the_daemons_cstan`, `launcher::wrapper::*`; "runs the CLI with the recorded node" is retired (the wrapper runs the daemon's `cstan`) |
| second launch reports the running PM; blocked PM; PM without a pane | replay `launch-pm`, `launch-pm-blocked-at-startup`, `launch-pm-without-a-live-pane`; live (second `launch` answers `running`, same pane) |
| blocked at startup keeps its pane row; hub opens with fallback off | replay `launch-pm-blocked-at-startup` |
| failed first start ends the agent and closes its pane | replay `launch-pm-start-fails` |
| crash between registering the PM and recording its pane (`needs_restart`) | replay `launch-pm-without-a-live-pane` |
| roles never synced | replay `launch-pm-not-synced` |
| crash between move and ledger write; adoption closes the moved pane | replay `adopt-closes-the-stray-of-an-interrupted-move`, `adopt-leaves-two-strays-alone`, `spawn-cleans-up-the-stray-of-an-interrupted-move` |
| restart: summary, old pane closed, new token, row consumed | replay `restart-pm`; live (`pm-restart` answers `blocked`, generation 2, new pane, old pane gone, worker pane stays) |
| restart: failed pane start leaves the row unconsumed | replay `restart-pm-start-fails` |
| restart: old pane that cannot be closed is an orphan | replay `restart-pm-old-pane-stays-open`, `restart-pm-old-pane-reused` |
| after a daemon restart panes are re-registered, lost ones cleared | replay `adopt-after-a-daemon-restart`, `adopt-a-restarted-pm`, `adopt-a-lost-pane`, `adopt-a-lost-pane-whose-id-was-reused`, `adopt-fails-for-another-reason`; live (second daemon re-adopts, delivery continues) |
| adoption sweeps stale rows and crashed spawns | replay `adopt-finishes-the-cleanup-of-an-ended-agent`, `adopt-the-watch-pane`, `spawn-fails-at-the-worktree` |
| project workspace that fails to open fails the PM start | replay `launch-pm-hub-fails`, `launch-pm-tab-fails-and-recovers` |
| hub not re-adopted for a transient reason; vanished watch pane made again | replay `hub-that-cannot-be-adopted`, `hub-whose-workspace-is-gone`, `hub-watch-tab-fails` |
| a pane Herdr no longer knows counts as closed | replay `release-pane-gone`, `release-pane-already-closed` |
| orphan pane kept across a daemon restart | replay `restart-pm-old-pane-stays-open` + `adopt-after-a-daemon-restart` |
| `env.pass` reaches the PM, workers and reviewers; unset variable named | replay `launch-pm-env-pass`, `spawn-env-pass`, `launch-pm-without-a-path-in-the-environment` |
| observe reads the recorded pane; `interrupt` needs the relay | replay `observe-an-agent`, `interrupt-needs-the-prompt-relay` |
| capture/answer prompt through the launcher | `daemon::cmds::prompt_show_records_the_capture_and_answer_types_it`, `prompt_show_of_an_unrecognised_dialog_offers_only_esc`, `prompt_answer_that_the_launcher_refuses_is_recorded_and_reported`; live `launcher::prompt_relay_live` |
| workspaces labelled with the project; panes report project, role and agent; adopted PM keeps the name | replay `launch-pm`, `adopt-a-restarted-pm`, `adopt-the-watch-pane` (the recorded adapter calls include the metadata reports) |
| `operatorEnvironment` is filtered, no `CAPSTAN_` variable | replay `launch-pm` (`operator_environment` step) |
| spawn never runs setup without a worktree configuration; a spawn failing while choosing the branch name cleans up | replay `spawn-setup-command`, `spawn-branch-names-are-free`, `spawn-fails-at-the-worktree` |
| **not ported** | two active PMs make launch refuse with `pm_exists`; a role whose definition changed is synced again on demand and a failing sync is named; a leftover row without a worktree path is reported as a record waiting to be cleaned up; a project path with a colon is refused; a failure to report metadata is logged and never fails a start; overlapping operations: only the one that started an agent reports the missing variable; restart whose replace the core refuses leaves the old pane and the ledger untouched; a hub made again while a PM runs closes its empty root pane; a PM start that fails before it takes the new workspace's root pane closes that pane |

### `launcher-spawn.test.ts` (52 tests)

| Behaviour | Disposition |
| --- | --- |
| spawn: worktree, token, prompt, worker profile, base sha, pane row | replay `spawn-tab-per-role`; live (worktree on `chore/developer-1-developer` at HEAD, pane, agent) |
| role not synced; unknown role, PM role, missing PM | replay `spawn-role-not-synced`, `spawn-refusals` |
| several workers up to the limit; the next spawn names who is active | replay `spawn-worker-limit`, `spawn-limit-names-the-stuck-cleanup`; live (`worker_limit` refusal) |
| pane mode: split into the PM's tab, width, stacking; stays a tab with the reason | replay `spawn-pane-layout`, `spawn-pane-no-room`, `spawn-pane-layout-fails`; live `launcher::live::pane_mode_splits_a_worker_into_the_pms_tab_and_release_closes_that_pane_and_removes_the_worktree` |
| a pane at the worktree path in another workspace is never closed | `launcher::operations::a_reused_pane_is_never_closed`; replay `teardown-of-a-reused-pane-id` |
| branch named for the generation; git must accept the name; given base commit | replay `spawn-branch-names-are-free`, `spawn-review-branch`, `spawn-special-roles`; `launcher::git::*` (`only_the_recorded_branch_and_a_legacy_capstan_branch_are_forced`); `launcher` unit `shared::tests::branch_names_follow_the_conventions` |
| a freed seat is reused; seat of another kind or disabled refused; orphan actor revoked | replay `spawn-tab-per-role`, `spawn-refusals` |
| trust dialog answered once with every key logged | replay `spawn-trust-dialog`; live (`daemon:trust_dialog_key` events, worker reaches idle) |
| failed spawn ends the agent, closes the pane, removes worktree and branch; retry works | replay `spawn-fails-at-the-worktree`, `spawn-retries-the-start` |
| operations run one at a time; concurrent spawns at a limit of one | `launcher::operations::one_operation_runs_one_waits_and_the_next_is_refused_as_busy`; replay `spawn-worker-limit` |
| Codex and OMP hosts | replay `spawn-hosts`, `launch-pm-codex`, `launch-pm-omp-with-architect` |
| Supervisor, Architect, Operator, Researcher roles and prompts | replay `spawn-special-roles`, `spawn-special-roles-count`, `spawn-operator-disabled`, `spawn-operator-without-a-table` |
| setup command once in the new worktree; failure rejects with `worktree_setup_failed`; time not charged to the budget; process group killed on timeout; filtered environment | replay `spawn-setup-command`; `launcher` unit `setup::tests::*` (`a_command_that_succeeds_is_ok`, `a_failing_command_reports_its_exit_code_and_output`, `a_command_past_its_time_ends_with_its_group`, `credentials_never_reach_the_tail`, `the_tail_keeps_the_end`) |
| a failed spawn names the failing step and the real error; start retried only for an unready prompt or shell | replay `spawn-describe-fails`, `spawn-retries-the-start`; `launcher` unit `spawn::tests::a_failed_step_names_itself_and_keeps_the_herdr_code` |
| `--task` names the branch; `renameBranchForTask` variants; task and title on the pane row | replay `rename-branch-for-task`, `spawn-terminal-id-unknown` (task columns appear in the ledger diff of `spawn-tab-per-role`); live: not driven |
| a title with a control character is refused | `launcher` unit `text::tests::task_text_rules` |
| `replace` gives the successor the predecessor's task and branch | replay `replace-with-seed`, `replace-explicit-branch` |
| retired | `runSetupCommand` / `defaultGit` as Node functions (the Rust ones are the unit tests above and `launcher::git`); "a worktree section loaded from `capstan.toml` reaches `runSetup`" (Node wiring; config parity is `config::config_parity`) |
| **not ported** | a freed seat's "dotted display name" for an extra seat; the Architect prompt differing from a developer's / "without the Architect no prompt mentions plans" as prompt-text assertions (the prompts are byte-compared inside the replayed `calls`, but only for the roles a sequence starts); the researcher allow-rule check; the PM keeping `pm_width_percent` when the first worker is placed |

### `launcher-release.test.ts` (38 tests)

| Behaviour | Disposition |
| --- | --- |
| release ends a worker, closes its pane, removes worktree and unchanged branch, frees the slot | replay `release-clean`; live (`released`, pane closed, worktree gone) |
| keeps a branch with commits; reports a worktree git refused to remove | replay `release-keeps-a-branch-with-commits`, `release-worktree-removal-refused`, `release-dirty-worktree` |
| keeps pane row, worktree and branch when the pane will not close; next operation finishes the job | replay `release-leaves-an-open-pane-for-the-next-start` |
| placed worker's new pane closed; pane lost in the move; cleanup leaves ambiguous panes alone | replay `release-pane-mode`, `adopt-leaves-two-strays-alone` |
| refusals: unknown agent, the PM, already released; cleanup that cannot end the agent | replay `release-refusals`, `spawn-limit-names-the-stuck-cleanup` |
| cleanup never forces; status lists an unfinished cleanup and retries it | replay `release-worktree-removal-refused`, `adopt-finishes-the-cleanup-of-an-ended-agent`; `launcher::git::a_capstan_worktree_with_untracked_files_is_removed_and_a_locked_or_unknown_one_is_reported` |
| `replace`: seeded from the last accepted report; moves work packages; falls back to HEAD; ended agent; old pane will not close; refusals; names the released agent when the replacement cannot start | replay `replace-with-seed`, `replace-without-a-report`, `replace-an-ended-agent`, `replace-leaves-a-pane-it-cannot-close`, `replace-refusals`, `replace-whose-start-fails`, `replace-in-another-host`, `replace-explicit-branch`; live (`replace` answers the successor, same branch, predecessor worktree removed) |
| replay of the seed text | `launcher` unit `release::tests::the_seed_labels_recorded_text_as_data`, `a_seed_over_the_limit_drops_the_oldest_messages_first` |
| replace continues the predecessor's branch and saves unreported commits at `refs/capstan/kept` | replay `replace-with-seed`; `launcher::git::only_the_recorded_branch_and_a_legacy_capstan_branch_are_forced` (`save_ref`) |
| teardown: once per cleanup in the project root, filtered environment, failures logged, real runner kills the group, not while the pane is open, runs again on retry, none without a worktree path | `launcher::operations::teardown_runs_the_configured_command_in_the_project_root`; replay `teardown-with-a-matching-terminal`, `teardown-commands-that-fail`, `teardown-runs-once-for-a-worktree-that-exists`, `teardown-after-a-pane-move` |
| an ended agent's pane id reused by a newer agent's pane is never closed | replay `teardown-of-a-reused-pane-id`, `teardown-of-a-pane-whose-tokens-name-another-agent`, `teardown-of-a-pane-of-another-project`, `teardown-without-a-recorded-terminal-id` |
| a worktree whose directory is gone counts as removed | replay `release-worktree-already-removed` |
| retired | "the documented codebase-memory teardown removes exactly the index files of its own worktree" (a shell snippet from the docs, run by Node's test; it asserts nothing about the launcher) |
| **not ported** | "replace and PM restart rebuild the researcher prompt and the research section"; "a timed-out setup rejects with `worktree_setup_failed` and the same cleanup" as a launcher-level case (the setup runner's timeout is the unit test `setup::tests::a_command_past_its_time_ends_with_its_group`); "teardown time is not charged to the cleanup budget" |

### `launcher-wrapper.test.ts` (10 tests)

Ported: `launcher::wrapper::without_node_variables_the_wrapper_only_runs_the_cstan_it_was_given`,
`the_node_variables_are_passed_on_only_when_the_daemon_has_them`,
`the_wrapper_quotes_paths_and_passes_the_arguments_and_variables_through`,
`the_wrapper_does_not_take_node_variables_from_the_agent_shell`; end to end `launcher::wrapper_handoff::an_agent_shell_runs_config_check_through_the_wrapper`
(now a normal test: `config check` is native, so it needs no Node); unit `shared::tests::the_wrapper_runs_the_cstan_and_passes_the_node_variables_only_when_set`.
Retired: the front-end selection (`frontEndPath`, `CSTAN_FRONT_END`, sibling `cstan`, SEA self-links): the wrapper always runs
the `cstan` the daemon was started as, so there is nothing to select.

### `launcher-git.test.ts` (9 tests)

Ported: `launcher::git::a_sha256_repository_is_named_as_unsupported_instead_of_reported_as_having_no_commit`,
`the_git_requirement_names_each_thing_that_is_missing`,
`a_capstan_worktree_with_untracked_files_is_removed_and_a_locked_or_unknown_one_is_reported`,
`only_the_recorded_branch_and_a_legacy_capstan_branch_are_forced`, `a_branch_is_deleted_only_while_it_is_at_the_recorded_commit`,
`a_failing_git_never_reads_as_no_worktree`. Covered by replay: "a failed start leaves no worktree and no branch" (`spawn-fails-at-the-worktree`),
"release logs how many files a worktree holds" (`release-dirty-worktree`), "cleanup never deletes a branch the ledger records for
another active agent" and the legacy `capstan/<agent>-g<n>` agent (`replace-with-seed`, `release-keeps-a-branch-with-commits`).
Retired: "no source file builds a branch from an agent id or a generation" (greps Node sources; the Rust names are
`shared::tests::branch_names_follow_the_conventions`).

### `launcher-live.test.ts` (2 tests)

Ported: `launcher::live::the_launcher_starts_a_pm_and_a_worker_delivers_readopts_after_a_restart_and_keeps_the_worker_through_a_pm_restart`
and `launcher::live::pane_mode_splits_a_worker_into_the_pms_tab_and_release_closes_that_pane_and_removes_the_worktree`; the same
operations plus mail, the PM wake, `replace`, `release` and `stop` in `daemon::live::the_rust_daemon_runs_a_project_in_a_real_herdr_session_and_leaves_nothing_behind`.
Branch renaming on a task bind (`renameBranchForTask`) is not driven live (replay `rename-branch-for-task`).

### `researcher-live.test.ts` (1 test)

Ported, opt-in: `launcher::researcher_live::a_spawned_researcher_has_the_playwright_mcp_connected_and_runs_a_read_only_curl_pipeline_with_no_permission_prompt`
(config check of the starter with the researcher blocks, spawn, `/mcp` shows playwright connected, the curl pipeline runs with
no permission prompt, a browser visit leaves no `.playwright-mcp`). Needs `CSTAN_LIVE=1`, `CAPSTAN_LIVE_RESEARCHER=1`, claude, npx,
jq and the network; the agent (not the test) runs `npx` and `jq`.

## Herdr

- `herdr-live.test.ts` — ported: `herdr::live_agent::the_adapter_drives_a_real_isolated_session_with_a_stand_in_agent` (version,
  `notification_not_shown`, clean agent shell, trust dialog keys, deferral while blocked, guarded sends incl. `--help` and
  multi-line, args log, input line deferral and `clear_after_deferral`, the PM pane never typed into, worktree removal);
  the existing `herdr::live::the_adapter_drives_a_real_session_and_removes_it`.
- `herdr-runner.test.ts` (10 tests) — covered: `herdr::runner::*` (session named, `HERDR_` dropped, exit code and streams, timeout and
  output cap, non-UTF-8, binary that cannot start, `run_json`), `herdr::parity::runner_helpers_match_node`,
  `a_failed_herdr_call_is_the_error_the_runner_made`, replay `herdr/tests/parity/runner.json`.
- `panes.test.ts` (15 tests) — covered by the kernel replay (`kernel/tests/replay.rs`): `agent-panes`,
  `agent-pane-of-ended-agent`, `pane-reuse-and-terminal-id`, `fallback-pane`, `orphan-panes`, `pm-restart-records`,
  `restart-pm-generation`, `end-agent-options-and-seed` (pane rows, intent rows, the fallback pane, restart summaries and
  their consumption, orphan panes, task columns). **Not ported:** the restart-summary budget and truncation tests (a summary
  always fits its budget, a very large task brief is shown as a preview, a carried body already cut is not cut again, a body
  that merely ends in the marker is still cut, cuts at a joined character): no kernel sequence records a truncated body.
- `process-activity.test.ts` — covered: `herdr::parity::the_tool_process_walker_behaves_like_node`,
  `the_activity_tracker_behaves_like_node`, `the_probe_reads_the_shell_pid_and_counts_tool_processes_like_node`,
  `process_parsers_match_node` (ps and `/proc` parsers incl. the macOS capture), `reading_the_real_proc_table_finds_this_process`.

## Prompt relay

- `prompt-relay-live.test.ts` — ported: `launcher::prompt_relay_live::a_real_claude_worker_blocked_on_a_permission_prompt_is_captured_and_answered_and_a_stale_relay_types_nothing`
  (real Claude Code; capture with `cstan prompt show`, answer Yes with the hash, an old relay is refused, text through the open
  field, the refused command does not run).
- `prompt-relay-commands.test.ts` (20 tests) — covered by `daemon::cmds::prompt_show_records_the_capture_and_answer_types_it`,
  `prompt_show_of_an_unrecognised_dialog_offers_only_esc`, `prompt_answer_that_the_launcher_refuses_is_recorded_and_reported`
  and the daemon transcripts `relay.json` (`prompt-checks`, `prompt-not-configured`); ported against the real daemon:
  `launcher::live::the_prompt_commands_refuse_who_may_not_use_them_and_what_is_not_a_prompt_and_type_nothing` (not a prompt,
  who may show, argument shapes, unknown relay, nothing typed, `promptRelay` in status) and
  `without_prompt_relay_both_prompt_commands_say_not_configured_and_status_has_no_section`. **Not ported:** a second show
  superseding the older capture, `relay_in_progress`, the several-text-options and no-text-option refusals, the
  client-disconnect case, the last-five answers in status, the dialog capture's TTL, the "Agent blocked" notice wording
  (the ledger rules behind them are replayed by the kernel `captured` sequences; the command-level cases need a scripted
  launcher in the daemon, which `daemon/tests/cmds.rs` owns and this package does not).
- `prompt-relay-ledger.test.ts` (9 tests) — covered by the kernel replay of `captured` (26 sequences: capture rules, TTL,
  hash prefix rule, `beginPromptAnswer`, immutability triggers, size limits) and `ledger::compat` (every migration version
  through the newest, so the registered-through-0037 test is `every_migration_file_is_embedded_with_the_checksum_of_its_bytes`).
  **Not ported:** an existing ledger that lacks 0030 migrates, keeps its data and backfills the PM grant (the schema of every
  version is checked; the 0030 data backfill is not).

## Operator

- `operator-restart-helper.test.ts` (18 tests) and `operator-restart-sea.test.ts` (the binary helper) — ported in
  `operator::restart`: `the_helper_backs_the_ledger_up_starts_the_new_build_and_reports_ok` (healthy path, backup),
  `a_new_build_that_fails_to_start_is_rolled_back_to_the_known_good_binary` (rollback, failed build kept),
  `a_ledger_migrated_past_the_known_good_build_is_restored_before_the_rollback`,
  `when_neither_build_starts_the_helper_reports_down_with_manual_recovery`,
  `a_total_failure_names_the_dependency_change_and_tries_the_restored_build_at_most_twice`,
  `the_helper_waits_for_the_old_controller_to_go_before_it_starts_the_new_build`,
  `a_stuck_old_controller_gets_sigterm_then_sigkill_and_only_its_own_stale_pid_file_is_removed`,
  `the_pid_file_of_another_live_process_is_never_removed_and_no_new_controller_is_started`,
  `a_handoff_failure_with_the_socket_held_by_something_that_is_no_controller_writes_down`,
  `a_handoff_failure_while_the_recorded_old_pid_still_answers_ping_is_rolled_back`,
  `a_handoff_failure_while_a_different_process_answers_ping_is_down_and_says_so`,
  `an_unreadable_key_file_still_writes_a_down_result_with_the_manual_recovery`, `an_unreadable_plan_is_a_down_result_not_a_hang`
  (and the plan without `depsChanged`: Node fails on the missing key and says "failed unexpectedly"; the Rust plan reads it as
  no change, the outcome is the same `down`). End to end: `daemon::live::an_operator_restart_replaces_the_daemon_and_the_clients_reconnect`
  (`op propose --restart`, `op decide`, the new `cstan daemon` answers, the ledger passes `integrity_check`, the front end
  reconnects). Retired: the helper's ping frame equals the Node client's (the frame is `capstan-wire`'s, byte-compared in
  `wire::parity`), "the helper is a standalone file that imports only node builtins" (a Node packaging rule), `directPing` against
  the Node daemon harness.
- `operator-runner.test.ts` (20 tests) — ported in `operator::runner`: the three sanitising tests, `a_run_reports_exit_codes_signals_and_the_output_of_both_streams`,
  `a_run_starts_in_its_directory_and_hands_the_command_text_to_sh_unchanged`, `a_run_gets_exactly_the_environment_it_is_passed`,
  `a_command_that_outlives_its_timeout_is_stopped_with_its_children`, `a_process_that_ignores_sigterm_is_killed_after_the_grace_period`,
  `output_above_the_cap_keeps_the_end_and_the_secrets_of_the_environment_are_redacted`, `a_command_that_cannot_start_reports_an_error`,
  `aborting_a_run_stops_its_process_group`, `on_spawn_reports_the_group_leader_before_the_command_ends`, and against the service
  with the real runner: the approved command in the project root with the controller's environment and the result message, the
  configured timeout, one megabyte of output, one run at a time, approval by the operator credential refused, the three
  stray-process-group cases (`a_stray_process_group_of_a_run_abandoned_at_startup_is_killed_and_approvals_wait_until_then`,
  `while_a_stray_group_cannot_be_confirmed_gone_..._the_tick_checks_again`, `a_pid_reused_by_another_process_is_never_signalled`).

## Ledger

The Node-made inputs are the committed fixture `test/fixtures/ledger-better-sqlite3.sqlite` (a v31 ledger with rows) and the
ledger of every version `0..=latest`, built the way Node's `openDatabase` builds one (each migration file run as it is, then
its `schema_migrations` row) in plain SQLite calls, not through the runner under test. No Node runs and no new fixture is
generated.

- `ledger-compat.test.ts` — ported in `ledger::compat`: `the_better_sqlite3_fixture_opens_verifies_and_reads_back_field_for_field`
  (rows, checksums verified against the files, the pre-existing rows unchanged, only the pending migrations back up, header
  bytes 16-24 and `user_version` unchanged, `integrity_check`, `foreign_key_check`, WAL), `the_fixture_opens_read_only_without_migrating`,
  `a_ledger_whose_writer_keeps_uncheckpointed_wal_frames_copies_and_opens_with_an_unchanged_header`,
  `a_ledger_with_integration_branches_keeps_every_row_trigger_and_index_through_the_migrations`. Retired: "a copy of the live
  ledger opens and migrates" (reads the operator's own `.capstan`; the machine rules forbid it).
- `migration-backups.test.ts` — the pruning cases are `ledger::migrate::prune_orders_by_version_then_timestamp_and_touches_nothing_else`,
  `prune_reports_an_unreadable_directory_instead_of_failing`, `keep_option_and_prune_error_handler`,
  `fresh_database_gets_every_migration_and_a_second_open_changes_nothing`, `upgrade_backs_up_before_each_step_and_keeps_the_newest_three`;
  ported in `ledger::compat`: `migrating_the_fixture_prunes_the_backups_it_took_to_the_configured_count`,
  `migration_0033_gives_every_existing_message_action_needed_0_and_is_backed_up_first`,
  `migration_0034_adds_task_columns_as_null_and_its_checks_reject_bad_values`,
  `migration_0037_adds_terminal_ids_as_null_and_its_checks_reject_bad_values`,
  `every_migration_file_is_embedded_with_the_checksum_of_its_bytes` (no number twice). The "failing unlink is logged" case
  is `migrate::keep_option_and_prune_error_handler`.
- `ledger-rust-interop.test.ts` — ported in `ledger::compat`: `a_ledger_written_by_node_at_every_version_migrates_to_the_schema_of_a_straight_run`
  (every `vN` including 0: identical `sqlite_master`, identical `schema_migrations` rows and checksums, `user_version`, the
  same newest-three `pre-vN` backups), the fixture tests above, `a_ledger_created_by_rust_is_what_node_would_find_current`
  (rows match the files, ISO times, a second open changes nothing and writes no backup). Covered by existing tests:
  refusals with Node's text (`ledger::refusals::*` over `tests/parity/refusals.json`), `open-ro` never creates a file
  (`migrate::read_only_never_creates_the_file`), integers beyond 2^53 (`database::integers_beyond_2_pow_53_are_refused`), the lock
  (`lock::*`, `daemon::replay::a_second_daemon_on_a_held_project_exits_4`). Retired as Node-internal: the staleness test of the
  committed fixtures against a fresh Node export (Node tooling), reading a Rust ledger back in Node, and the lock tests in which
  a Node process holds or races the lock (no Node daemon takes part any more).
- `sqlite-adapter.test.ts` — covered by `ledger::database::*` (`transactions_nest_through_savepoints`,
  `a_panic_in_a_transaction_rolls_back`, `statement_cache_holds_256_texts_and_pragma_returns_rows`, `backup_copies_the_database`,
  `integers_beyond_2_pow_53_are_refused`) and `ledger::migrate::fresh_database_gets_every_migration_and_a_second_open_changes_nothing`
  (WAL, busy timeout, foreign keys, `synchronous=FULL`, read-only open); the Node `node:sqlite`/`better-sqlite3` adapter itself is
  retired.

## Deferred

- The hub's watch pane runs `cstan status --watch`, which the front end does not serve yet (it hands it to Node). The harness
  sets `CSTAN_NODE_CLI` to a stand-in that prints `WATCH-PANE-RUNNING` and stays up for exactly that command and exits 5 for
  any other (so a command handed to Node fails the test at once). Without it the watch pane exits, and Herdr then refuses to
  close the PM's pane in `pm-restart` ("closing this pane would close a worktree group"). Plan B's `suites-core` replaces the
  stand-in when `status --watch` is native.
- `daemon::live::an_operator_restart_...` draws one frame of `cstan-dash` when the binary is beside `cstan`; `cstan-dash` is built
  from `dash/`, apart from the workspace, so the step reports that it did not run when it is not built.
- `launcher::researcher_live` and `launcher::prompt_relay_live` need local commands and a login (claude, and for the first also
  npx, jq and the network); they are opt-in and not part of `cargo test`.

## Not ported

Behaviour with no replay and no Rust test (searched by error code and phrase in the launcher, daemon and kernel corpora),
so it is unchecked on the Rust side until a fixture or a Rust-only test is added (a new Node export is not allowed by this
package; the files are outside its ownership):

1. `launcher-lifecycle`: `pm_exists` with two active PMs; on-demand role re-sync and its failure text; the leftover row without
   a worktree path; the project path with a colon; the logged metadata failure; the missingEnv attribution across overlapping
   operations; a restart whose replace the core refuses; the empty root pane of a hub made again; a PM start failing before it
   takes the root pane.
2. `launcher-spawn`: the dotted display name of an extra seat; prompt-text assertions for Architect/plans; the researcher allow
   rule check; `pm_width_percent` kept when the first worker is placed.
3. `launcher-release`: researcher prompt rebuilt by replace and PM restart; teardown time not charged to the cleanup budget.
4. `panes`: the restart-summary budget and truncation cases.
5. `prompt-relay-commands`: supersede, `relay_in_progress`, text-option ambiguity, disconnect during answer, status last five,
   dialog TTL, blocked-notice wording.
6. `prompt-relay-ledger`: the 0030 migration with data (PM grant backfill).
