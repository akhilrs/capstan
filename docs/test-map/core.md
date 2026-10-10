# Test map: the core suites (plan-30 `suites-core`)

Where every Node `test/*.test.ts` and `test/*.test.tsx` file went that `docs/test-map/live.md` does not classify and that
`release.md` does not take (`release-notes`, `packaging`, `sea`, `operator-restart-sea`). Each file is **ported** (Rust test
names), **covered by a named replay** (a frozen corpus replayed by Rust: the sequence or group is named; a replay covers a
behaviour area, not each assertion one for one) or **retired as Node-internal** (the reason). `docs/test-map/live.md` has the
launcher, Herdr, operator-restart, prompt-relay, ledger-compat and researcher-live files.

How to read a name: `crate::file::test` is `rust/crates/<crate>/tests/<file>.rs`. The black-box crate is
`capstan-blackbox` (`rust/crates/blackbox`).

## The black-box suites (`capstan-blackbox`)

A test-only crate. Every test runs the **built `cstan`** as a process (the release build when `target/release/cstan` exists,
else the profile of the run; `CSTAN_BIN` names one) in a scratch project under `/tmp/capstan-suites-core-*`, with `PATH` set to a
directory that holds links to `git`, `sh` and a few coreutils and **no node**; `cstan daemon` runs as a child it owns, is stopped,
reaped and removed in `Drop`, also when the test fails; `cstan-dash` is drawn once under a terminal when `CSTAN_DASH_BIN` names a
build of it (it builds in `dash/`, apart from the workspace). Agents' tokens come from the ledger, seeded through the kernel
before the daemon starts (seats, actors, agents, pane rows: what a spawn leaves), and are used with `CAPSTAN_TOKEN` and
`CAPSTAN_SOCKET` as an agent shell has them. Herdr is not involved (`CAPSTAN_LAUNCH=off`, or a launching daemon with no `herdr`
on its PATH); the one thing a launcher does that a test cannot is a reviewer's spawn, so a review is begun in the ledger the way
`request-review` does after it (`World::begin_review`), and every verdict, integration, sign-off and confirm after that is the
real command.

```
cargo test --locked -p capstan-blackbox            # about 50 s with the test build; no node on PATH
CSTAN_DASH_BIN=<path of a built cstan-dash> cargo test --locked -p capstan-blackbox --test status   # adds the dash frame
cargo test --locked -p capstan-blackbox -- --ignored   # the known defect below
```

| Node files | Black-box tests (`capstan_blackbox::tests::<file>::<test>`) |
| --- | --- |
| `cli.test.ts`, `commands.test.ts` (CLI and start/stop part), `daemon-select.test.ts` (start part) | `cli::`: `version_help_and_usage_errors_exit_with_their_codes`, `a_folder_that_is_not_a_repository_or_has_no_commit_is_named_with_the_exact_fix`, `the_scratch_environment_has_no_node_and_the_native_cli_needs_none`, `init_creates_a_private_project_a_starter_config_and_refuses_to_change_it`, `init_keeps_an_existing_capstan_toml_and_a_dangling_symlink`, `config_check_exits_3_for_a_missing_or_invalid_file_and_never_echoes_a_secret`, `config_check_warns_about_roles_that_run_unattended_on_another_host`, `config_sync_writes_the_roles_once_and_a_second_run_writes_nothing`, `start_ping_and_stop_manage_one_daemon_per_project`, `sigterm_stops_the_daemon_cleanly_and_the_next_start_needs_no_recovery`, `after_kill_9_status_still_reads_the_ledger_and_start_replaces_the_dead_daemons_files`, `a_broken_capstan_toml_stops_the_daemon_at_start_with_the_loaders_message`, `a_restrictive_umask_does_not_stop_a_start_and_the_log_stays_private`, `agent_mode_uses_the_token_and_the_socket_from_the_environment`, `a_bad_token_is_named_and_routed_arguments_keep_a_literal_json_after_the_separator` |
| `daemon.test.ts`, `daemon-launch.test.ts`, `daemon-operator.test.ts`, `daemon-background.test.ts`, `daemon-cost.test.ts` | `daemon::`: `the_operator_credential_is_for_operator_and_read_commands_and_an_agent_token_for_agent_commands`, `a_missing_malformed_or_wrong_credential_is_unauthorized_for_every_command`, `tokens_never_appear_in_status_the_log_or_any_file_and_the_log_holds_no_arguments`, `the_socket_is_private_and_the_state_directory_too`, `a_stale_socket_of_a_dead_daemon_is_replaced_and_a_file_or_link_at_the_path_is_left_alone`, `frames_are_limited_to_64_kib_a_newline_ends_one_and_bad_input_is_answered`, `a_frame_may_arrive_in_pieces_and_a_silent_connection_is_dropped_after_five_seconds`, `shutdown_needs_the_operator_and_ends_the_daemon_leaving_no_socket_or_pid_file`, `a_second_daemon_on_a_held_project_does_not_start`, `without_a_launcher_the_management_commands_say_not_configured`, `a_launching_daemon_without_herdr_keeps_its_pane_less_pm_and_stays_up`, `without_operator_enabled_op_is_not_configured_and_no_restart_directory_exists`, `an_idle_daemon_writes_nothing_to_its_log_and_status_inbox_and_inspect_agree_on_a_populated_ledger` |
| `commands.test.ts` (messages), `messages.test.ts`, `messaging.test.ts` (CLI-visible rules), `stale-reports.test.ts` | `messages::`: `the_pm_inbox_prints_everything_pending_and_an_ack_works_in_any_order`, `a_worker_inbox_pulls_its_mail_in_order_and_the_operators_peek_changes_nothing`, `send_is_limited_workers_write_to_the_pm_nobody_to_themselves_and_bad_recipients_are_refused`, `a_body_that_imitates_a_capstan_frame_or_is_too_large_is_refused_and_nothing_is_queued`, `two_active_pms_make_at_pm_ambiguous_and_no_pm_leaves_the_name_unknown`, `an_ack_resolve_and_cancel_move_a_message_and_a_retry_from_sent_carries_a_warning`, `send_action_marks_the_frame_and_the_unread_notice_names_the_count`, `a_wait_returns_queued_mail_at_once_and_a_message_that_arrives_ends_a_running_wait`, `a_wait_times_out_with_the_hosts_limit_and_only_an_agent_waits_without_arguments`, `a_newer_wait_supersedes_the_older_one`, `a_client_that_is_killed_ends_its_wait_and_nothing_is_acked`, `an_ended_agents_token_is_refused_and_a_message_queued_before_the_end_is_cancelled`; `reports::after_a_replacement_the_old_token_is_refused_and_the_replacement_cannot_report_the_predecessors_commit` |
| `pause.test.ts` | `pause::`: `the_operator_and_the_pm_pause_and_resume_a_worker_and_a_worker_may_not`, `a_run_pause_holds_new_work_and_everything_else_keeps_working`, `a_supervisor_cannot_raise_a_finding_against_a_paused_agent`, `pauses_survive_a_restart_of_the_daemon_and_ending_an_agent_closes_its_pause`, `interrupt_needs_the_prompt_relay_and_pauses_nothing_without_it` |
| `findings.test.ts`, `finding-commands.test.ts` | `findings::`: `a_supervisor_raises_a_finding_and_the_delivery_reaches_the_worker_and_the_pm_is_told`, `only_an_active_supervisor_raises_and_only_about_another_active_worker`, `a_check_waits_for_the_ack_and_only_the_raising_supervisor_checks`, `an_unresolved_check_sends_a_second_correction_and_a_second_unresolved_check_escalates`, `ten_finding_attempts_a_minute_are_allowed_per_supervisor_and_the_eleventh_is_refused_for_the_rate`, `observe_is_for_the_pm_and_a_supervisor_and_needs_a_launcher` |
| `plan-flow.test.ts`, `plan-routing.test.ts`, `plan-dependency.test.ts`, `plan-review-commands.test.ts`, `plans-core.test.ts` (command surface) | `plans::`: `a_normal_plan_is_approved_at_once_and_the_pm_is_told`, `plan_commands_check_who_and_what`, `plan_assign_sends_one_message_binds_the_package_and_names_the_branch`, `a_package_whose_dependency_is_not_reviewed_is_refused_and_early_needs_a_reason`, `the_whole_path_from_an_open_plan_to_the_pms_confirm`, `a_developer_writes_to_the_architect_and_the_architect_to_a_developer_and_nothing_else_opens_up`, `with_architect_disabled_a_developer_writes_only_to_the_pm_and_plan_commands_are_refused`, `a_report_from_an_assignee_goes_to_the_architect_with_the_package_line_and_anyone_elses_goes_to_the_pm`, `a_high_risk_plan_that_needs_a_reviewer_who_cannot_start_stays_a_draft_and_tells_the_architect` |
| `reports.test.ts`, `review-commands.test.ts`, `reviews.test.ts`, `integration.test.ts`, `integration-git.test.ts` | `reports::`: `an_accepted_report_is_recorded_and_told_to_the_pm_and_a_repeat_writes_nothing`, `each_way_a_claim_can_fail_is_rejected_with_its_reason`, `new_commits_that_break_the_commit_rules_are_refused_and_accepted_once_fixed`, `a_worker_is_limited_to_ten_report_commands_a_minute`, `a_report_is_refused_while_mail_from_before_the_commit_is_unread_and_a_later_message_does_not_block`, `a_reviewers_verdict_is_for_a_reviewer_with_a_review_and_a_new_round_is_a_new_session`, `integrate_needs_reviewed_reports_merges_them_into_one_branch_and_runs_one_at_a_time`, `a_conflict_names_the_report_and_the_files_and_leaves_no_branch_or_changed_file`, `repository_hooks_do_not_run_during_an_integration`, `a_confirmed_integration_cannot_be_integrated_again` |
| `operator-commands.test.ts`, `operator-full-auto.test.ts`, `operator-grants.test.ts` | `operator::`: `without_an_enabled_operator_every_op_command_answers_not_configured_and_writes_nothing`, `only_the_designated_operator_agent_proposes_and_arguments_are_checked`, `the_pm_approves_with_the_exact_hash_and_the_command_runs_in_the_project`, `deny_and_cancel_end_a_proposal_that_has_not_run`, `only_the_pm_writes_to_the_operator_agent_and_the_operator_agent_writes_only_to_the_pm`, `full_auto_is_switched_on_by_the_pm_with_the_users_words_and_approves_proposals_at_propose_time`, `full_auto_is_not_kept_across_a_restart_of_the_daemon`, `a_session_grant_lets_an_identical_command_run_without_a_new_approval_and_revoking_ends_it`, `a_restart_proposal_is_refused_when_no_restart_is_available` |
| `status-active-tasks.test.ts`, `status-feed.test.ts`, `nexora-commands.test.ts`, `watch.test.ts`, `dash-poller.test.ts`, `dash-safety.test.ts` | `status::`: `link_commands_are_for_the_pm_and_the_operator_and_refuse_bad_input`, `plan_show_prints_the_links_and_only_the_pm_status_carries_the_drift`, `plan_cancel_is_for_the_operator_and_tells_the_pm_and_the_architect`, `only_the_operator_gets_active_tasks_and_panes_and_cancelled_plans_are_absent`, `the_operator_status_carries_pipeline_totals_and_pending_proposals_only_with_the_operator_on`, `the_watch_frame_lists_the_agents_and_the_unresolved_messages_without_control_characters`, `the_watch_ends_when_the_controller_stops_answering`, `dash_needs_a_terminal_and_the_cstan_dash_binary_and_with_both_draws_a_frame` |
| `recovery.test.ts`, `recovery-core.test.ts` (what a restart does) | `recovery::`: `a_launching_daemon_ends_an_agent_that_has_no_pane_records_it_lost_and_tells_the_pm_once`, `after_kill_9_the_next_command_restarts_the_daemon_and_nothing_is_duplicated`, `a_wait_row_left_by_a_dead_daemon_is_closed_at_the_next_start` |

### Known defect (an `#[ignore]` test)

`cli::two_starts_at_once_end_with_one_daemon_and_both_succeed` (`cli.test.ts`, "two cstan start commands at once end with one
daemon and both succeed") fails about three runs in ten, release and debug build alike: both `cstan daemon` children lose the
project lock at once (`cstan: another cooperating controller owns this project` twice in the log, no `ready` line), no daemon
is left, and both clients end with exit 5 (`another controller holds the project lock but does not answer`). The probable cause is
`ProjectLock::acquire` (`ledger/src/lock.rs`): `BEGIN EXCLUSIVE` with a zero busy timeout returns BUSY to both processes when they
arrive together (Node has the same protocol). A short bounded retry on BUSY would fix it. The test is `#[ignore = "known defect:
concurrent start, follow-up fix pending"]`; un-ignore it as the proof when the fix lands. Reproduction without the test: in a
`cstan init`-ed git repository with `CAPSTAN_LAUNCH=off`, run two `cstan start --json` at the same time ten times (a fresh state
directory and an existing one both fail about three times in ten), `cstan stop` between rounds:

```
run() { env -i PATH=/usr/bin:/bin HOME=$PWD CAPSTAN_LAUNCH=off cstan "$@"; }   # in a repository after `cstan init` and one commit
for round in $(seq 10); do
  (run start --json >a.out 2>a.err; echo "a=$?" >>a.out) & (run start --json >b.out 2>b.err; echo "b=$?" >>b.out) & wait
  echo "round $round: $(tail -1 a.out) $(tail -1 b.out)"; run stop >/dev/null
done                                  # a failing round prints a=5 b=5
```

## Other Rust tests this package added

| Test | What |
| --- | --- |
| `herdr::live_home::*` (3) | the live harness's scratch HOME holds no symlink into the real one; the scratch `CLAUDE_CONFIG_DIR` is a private copy of the sign-in and nothing else |
| `daemon::server::*` (the concurrent and timeout groups) | compared by shape (fields and codes), not by order or elapsed time; a blocked launcher is held by a gate the test opens, not by a sleep |
| `daemon::client_interop::the_front_end_runs_against_the_rust_daemon` | only the Rust client half remains |
| `daemon::cmds::*` (seven prompt-relay cases) and `launcher::rust_only::*`, `kernel::rust_only::*` | the rows `live.md` listed as not ported (see `live.md`) |
| `launcher::wrapper`, `launcher::wrapper_handoff`, the parity engine's git shim, the live harness's `claude` script | scripts are written under a temporary name and renamed into place, and the tests that execute them hold one lock (`launcher/tests/common/exec.rs`): the "Text file busy" race of a parallel test's fork |

## Every other Node file

### Parity staleness and export tests (retired as Node-internal; the corpus they guard is replayed by Rust)

Each of these checks that the committed fixtures equal a fresh export from the Node implementation. After the freeze there is no
Node to export from; the fixtures are the corpus. The Rust replays are in `docs/parity-fixtures.md` (exporter, fixtures, replay).

| File | Rust replay |
| --- | --- |
| `cli-transcript.test.ts`, `cli-local-transcript.test.ts`, `cli-parity.test.ts` | `cstan/tests/transcripts.rs` over `transcripts/` and `local.rs` over `local-transcripts/` (including the daemon-command and offline-status transcripts) |
| `config-parity.test.ts` | `config/tests/config_parity.rs`, `prompts_parity.rs`, `differential.rs` |
| `daemon-transcript.test.ts` | `daemon/tests/replay.rs`, `cmds.rs`, `budget.rs`, `shadow.rs` over `transcripts/{agents,core,findings,links,messages,operator,plans,relay,reports,server,status,wait}.json` |
| `loops-parity.test.ts` | `daemon/tests/loops.rs` over `loops-parity/{driver,recover,relay,supervision}.json` |
| `kernel-parity.test.ts`, `kernel-integrate-parity.test.ts` | `kernel/tests/replay.rs`, `strict.rs`, `integrate.rs` |
| `launcher-parity.test.ts` | `launcher/tests/parity.rs` (`every_sequence_replays_with_the_node_launcher_outcome`) |
| `operator-parity.test.ts` | `operator/tests/parity.rs` over `parity/{policy,restart}.json` |
| `herdr-parity.test.ts` | `herdr/tests/parity.rs`, `adapter_replay.rs` |
| `dash-parity.test.ts` | `dash/tests/model_parity.rs`, `view_parity.rs`, `e2e_parity.rs` |
| `wire-parity.test.ts` | `wire/tests/parity.rs` |

### Covered by a named replay

| File | Disposition |
| --- | --- |
| `action-needed.test.ts` | kernel replay `messages`: `notices`, `missing-notices` (the `actionNeeded` flag of every controller notice); `daemon/tests/transcripts/messages.json` `send-basics` |
| `config.test.ts`, `designer-prompt.test.ts`, `researcher-config.test.ts`, `operator-config.test.ts`, `hosts.test.ts` | the config corpora: `config/tests/config_parity.rs` (resolution, defaults, refusals), `prompts_parity.rs` (the designer prompt bytes, host arguments for Claude, Codex and OMP), `differential.rs`; the researcher and operator policies: the same corpus plus black-box `cli::config_check_*` |
| `controller-roles.test.ts`, `controller-project.test.ts` | kernel replay `core`: `bootstrap`, `reopen`, `open-refusals`, `second-open-and-read-only`, `actors`, `role-definitions`, `idempotency`, `conflicts-and-validation`, `credentials`; black-box `cli::config_sync_*` for the sync |
| `messages.test.ts` (ledger rules), `messaging.test.ts` (timers, partition of states), `oversight.test.ts` | kernel replay `messages`: `delivery-flow`, `timers`, `notices`, `supervision`, `missing-notices`, `waits`; loops replay `driver`: `delivery-sent`, `busy-and-blocked`, `pm-wake`, `pm-notify`, `pm-stale`, `stall-and-process-activity`, `blocked-attention`, `paused-agent` |
| `driver.test.ts` | loops replay `driver` (all 22 sequences: delivery, deferral, clears, send failures, pane mismatch, agent lost, Herdr restart, wake, notify, stall, blocked, pause, adapter errors) |
| `supervision.test.ts` | loops replay `supervision` (5 sequences) |
| `plans.test.ts` (the plan body parser), `plans-core.test.ts` (ledger rules) | kernel replay `plans`: `plan-open-submit-review`, `plan-lifecycle`, `plan-notices-without-pm`, `plan-cancel`; `cross`: `plan-to-signoff-with-findings`; the plan-body fixtures `test/fixtures/plan-bodies` (`kernel/tests/replay.rs`) |
| `reviews.test.ts`, `reports.test.ts` (ledger rules), `integration.test.ts` (ledger rules), `integration-git.test.ts` (git rules) | kernel replay `plans`: `agent-reports`, `report-limit`, `integrations`; `parity-integrate`: `scenarios` (`clean-merge`, `conflict`, `conflict-paths`, `conflict-many`, `contained-and-missing`, `recover-interrupted`, `settle`, `sweep-at-start`, `plan-naming`, `plan-unicode`, `commit-subjects`, `coverage`) and `squash` (30 message cases); loops replay `recover` (6) |
| `recovery.test.ts`, `recovery-core.test.ts` | loops replay `driver`: `delivery-sent`, `agent-lost`, `herdr-restart-suppression`, `adapter-errors-then-recovery`, `no-pane`; `recover`: all six; kernel replay `agents`: `end-agent`, `end-agent-options-and-seed`, `replace-generation`; black-box `recovery::*` |
| `seed.test.ts` | launcher unit `release::tests::the_seed_labels_recorded_text_as_data`, `a_seed_over_the_limit_drops_the_oldest_messages_first`; replays `replace-with-seed`, `replace-without-a-report` |
| `prompts.test.ts` | the byte-compared prompts of the config corpus (`config/tests/prompts_parity.rs`) and of the launcher replays (`calls` of `launch-pm`, `spawn-tab-per-role`, `spawn-special-roles`, `restart-pm`); `launcher::rust_only::the_architect_prompt_differs_from_a_developers_and_the_pm_prompt_carries_the_plan_section`, `without_the_architect_no_prompt_mentions_plans`, `replace_and_pm_restart_rebuild_the_researcher_prompt_and_the_research_section` |
| `layout.test.ts` | launcher replays `spawn-pane-layout`, `spawn-pane-no-room`, `spawn-pane-layout-fails`; `launcher::rust_only::the_pm_keeps_pm_width_percent_of_its_tab_when_the_first_worker_is_placed` |
| `observe.test.ts`, `dash-peek.test.ts` | launcher replay `observe-an-agent`; herdr adapter replay (the sanitising of screens, `herdr/tests/adapter_replay.rs`); black-box `findings::observe_is_for_the_pm_and_a_supervisor_and_needs_a_launcher`; the peek overlay in `dash/tests/app_behaviour.rs` (`o_on_an_active_agent_peeks_through_a_call_and_esc_or_q_closes_the_overlay`, `a_refused_peek_is_a_notice_and_an_ended_agent_cannot_be_observed`) |
| `notifier.test.ts` | loops replay `driver`: `pm-notify`, `pm-notify-fallback-only`, `pm-stale` (the channels and their lines) |
| `naming.test.ts`, `task-text.test.ts`, `conventions.test.ts` | launcher units `shared::tests::branch_names_follow_the_conventions`, `text::tests::task_text_rules`; replays `rename-branch-for-task`, `spawn-branch-names-are-free`; `parity-integrate` `commit-subjects` and `plan-naming` |
| `git.test.ts`, `git-requirement.test.ts` | `launcher::git::*` (`the_git_requirement_names_each_thing_that_is_missing`, `a_sha256_repository_is_named_as_unsupported_instead_of_reported_as_having_no_commit`, `a_failing_git_never_reads_as_no_worktree`, the commit checks); black-box `cli::a_folder_that_is_not_a_repository_or_has_no_commit_is_named_with_the_exact_fix`; `reports::new_commits_that_break_the_commit_rules_are_refused_and_accepted_once_fixed` (new commit messages) |
| `status-query-cost.test.ts` | `ledger::database::statement_cache_holds_256_texts_and_pragma_returns_rows`, `ledger::migrate::*` (the indexes of migration 0036 are in the migration files that `ledger::compat::every_migration_file_is_embedded_with_the_checksum_of_its_bytes` checks) |
| `daemon-cost.test.ts`, `daemon-background.test.ts` | loops replay `driver` (`stall-and-process-activity`, `probe-failure`: the same Herdr calls, the process probe) and black-box `daemon::an_idle_daemon_writes_nothing_to_its_log_and_status_inbox_and_inspect_agree_on_a_populated_ledger`; the cost budgets are Node timings (retired) and the young-generation / `NODE_OPTIONS` tests are retired as Node-internal (below) |
| `live-daemon-guard.test.ts` | black-box `cli::agent_mode_uses_the_token_and_the_socket_from_the_environment` (foreign socket refused, `CAPSTAN_ALLOW_FOREIGN_SOCKET=1` warns once, a relative socket named) and `cstan/tests/binary.rs` (the hook on a foreign socket is silent) |
| `herdr-adapter.test.ts`, `herdr-adapter-input.test.ts`, `herdr-adapter-panes.test.ts`, `herdr-adapter-args.test.ts`, `herdr-screen.test.ts` | `herdr/tests/adapter_replay.rs` and `parity.rs` over `herdr/tests/parity/`; live `herdr::live_agent` and `herdr::live` (see `live.md`) |
| `controller-canonical.test.ts` | `kernel/src/canonical.rs` units and the kernel replay `cross`: `caller-json-order`, `argument-shapes` (the JS accessor and ill-formed UTF-16 cases cannot arise from JSON on the wire: retired as Node-internal) |
| `project-lock.test.ts` | `ledger::lock::*` and `daemon::replay::a_second_daemon_on_a_held_project_exits_4`; black-box `daemon::a_second_daemon_on_a_held_project_does_not_start`; the concurrent case is the known defect above |
| `dash-actions.test.ts`, `dash-model.test.ts`, `dash-ended-filter.test.ts`, `dash-tasks.test.ts`, `dash-tasks-contract.test.ts`, `dash-waiting.test.ts`, `dash-format.test.ts`, `dash-layout.test.ts`, `dash-draw.test.ts`, `dash-golden.test.ts` | `dash/tests/model_parity.rs` (the model of every fixture, incl. active tasks and the ended filter), `view_parity.rs` (every frame and overlay, the format tables, the layout modes and the palette), `core_helpers.rs` (actions, `clean`, the highlight polls), `e2e_parity.rs` (daemon to buffer, every size 1x1 to 200x60) |
| `dash-launch.test.ts` | `cstan/tests/dash_handoff.rs` (where `cstan-dash` is looked for, in order) |
| `operator-restart.test.ts`, `operator-ledger.test.ts`, `operator-policy.test.ts`, `operator-config.test.ts` (policy part) | the operator replays: `operator/tests/parity.rs` over `parity/restart.json` (busy snapshot, idle wait, restart outcomes) and `parity/policy.json` (the auto-approve policy), `operator::service`, `operator::restart` (the restart service and helper); the ledger rules of proposals, runs, grants and full auto: kernel replay `ops` (`operator-propose-validation`, `operator-decide`, `operator-runs`, `operator-run-guards`, `operator-grants`, `operator-expiry`, `operator-full-auto`, `operator-no-pm`, `operator-agent-end`); the command surface: black-box `operator::*` |
| `daemon-select.test.ts` | the selection (`[daemon] implementation`, `CSTAN_DAEMON`, `CSTAN_DAEMON_BIN`) is left to cutover (`plan-30/cutover`); there is one daemon and `cstan daemon` starts it (black-box `cli::*`, `daemon::*`) |

### Retired as Node-internal

| File | Reason |
| --- | --- |
| `check-dash.test.ts` | the Node `check:dash` script finding cargo; the Rust gate is `cargo test --workspace` |
| `lint-controller-imports.test.ts` | an ESLint rule about which Node modules may import `src/controller/*`; Rust has its crate boundaries |
| `dash-app.test.tsx` | the React/ink dashboard application; the dashboard is `cstan-dash` (tests in `dash/tests/app_behaviour.rs` are its counterpart, listed above) |
| the cost-budget and young-generation parts of `daemon-cost.test.ts` and `daemon-background.test.ts` | Node timings and `NODE_OPTIONS` handling of the Node daemon |
