# Operator reference

Back to the [README](../../README.md).

An optional **Operator** agent lets the PM have a shell command run, or the controller restarted, without you at the keyboard. The Operator only proposes; the controller runs. Nothing about the Operator exists while `[operator]` is absent or `enabled = false`: no role, no prompt text, no `op` command (it answers `not_configured`), no restart code loaded, no known-good snapshot taken, no `restart/` directory.

**Model.** The Operator is a Developer-kind role named by `[operator] role`. It runs `cstan op propose "<command>" "<reason>"` (the command first, then the reason; or `cstan op propose --restart [--force] "<reason>"`). The controller stores the proposal with the exact command text and a hash of kind, text and force flag, tells the PM, and runs approved proposals one at a time as the project user, in the project root. The runner passes the approved command text to `sh -c` (`rust/crates/operator/src/service.rs`), so the shell interprets that text (quoting, expansion, pipes). The text is exactly what was proposed and approved; it is never assembled from other fields. An active PM agent approves with `cstan op decide <id> approve --hash <hash12>`; the hash must match the stored proposal. The operator CLI (you, with `.capstan/operator.key`) can only `deny`, `cancel`, `show`, `revoke` a grant and switch full auto `off`.

**Keys and defaults** (`[operator]` in `capstan.toml`; unset keys take these defaults):

| Key                                       | Default        | Meaning                                                           |
| ----------------------------------------- | -------------- | ----------------------------------------------------------------- |
| `enabled`                                 | `false`        | Turns the Operator on.                                            |
| `role`                                    | `"operator"`   | Developer role that may propose.                                  |
| `auto_approve`                            | `[]`           | Exact read-only commands that skip the PM.                        |
| `auto_approve_prefix`                     | `[]`           | Opt-in prefixes; only safe path arguments may follow.             |
| `timeout_seconds` / `max_timeout_seconds` | `300` / `1800` | Time a command may run; the group is killed at the limit.         |
| `output_tail_bytes`                       | `8192`         | End of the output kept (at most 12288).                           |
| `proposal_ttl_minutes`                    | `60`           | A proposal nobody decided expires.                                |
| `approval_ttl_minutes`                    | `10`           | An approved proposal that did not start expires (`approval_ttl`). |
| `max_pending_proposals`                   | `5`            | Open proposals at once.                                           |
| `count_toward_worker_limit`               | `false`        | Whether the Operator counts as a worker.                          |
| `restart_health_timeout_seconds`          | `60`           | How long a restarted controller has to answer `ping`.             |
| `restart_idle_wait_seconds`               | `120`          | How long a restart waits for the controller to be idle.           |
| `session_grant_max_minutes`               | `60`           | Longest a session grant lasts (at most 480).                      |
| `timed_grant_default_minutes`             | `60`           | Length of `--session timed` (at most `timed_grant_max_minutes`).  |
| `timed_grant_max_minutes`                 | `240`          | Longest a timed grant lasts, also for `timed=session` (hard cap 480). |
| `timed_safe_commands`                     | built-in list  | Read-only commands a timed grant may cover; `[]` disables timed grants. |
| `full_auto_default_minutes`               | `30`           | Full auto duration when the PM gives none (at most the maximum).  |
| `full_auto_max_minutes`                   | `120`          | Longest full auto period (at most 480).                           |

**Auto allowlist.** Only exact commands that match `OPERATOR_AUTO_ALLOWLIST` in `rust/crates/operator/src/policy.rs` and are listed in `auto_approve` (or match an `auto_approve_prefix`) run without the PM: `ls` with `-l -a -la -h` and plain path arguments, `pwd`, `whoami`, `date`, `uname -a`, `df -h`, `cstan ping`, `cstan status`, `git rev-parse` (`--abbrev-ref`, `--short`, `HEAD`) and `git ls-files`. `git status`, `git diff` and `git show` are deliberately not on it: they can run project-configured helpers (`diff.external`, textconv, pagers), so they can execute project code. A restart is never auto-approved, even if you list it.

**Denylist.** A second layer, not the guard: a command containing words such as `push`, `rm`, `reset`, `checkout`, `merge`, `sudo`, `curl`, `ssh`, `sh`, `bash`, `eval`, `xargs`, `find`, `make`, `docker`, the names of script interpreters and package managers, or the short options `-f -d -D -x -r -R -o -O -e -c`, always needs the PM, whatever the rules say. The full list is `OPERATOR_ALWAYS_APPROVAL` in `rust/crates/operator/src/policy.rs`.

**PM approval.** The PM sees the exact command, the reason and the hash, shows you the command and decides. Approval is bound to the text, the kind and the force flag: a changed text has another hash. An approved proposal runs once; no approval is taken while a run is in progress. Command text and reason are printable ASCII only.

**Working directory and environment.** Commands run in the project root. The child gets no `CAPSTAN_` variable and no `cstan` wrapper on its `PATH`.

**Output sanitisation, and its limits.** The output tail has terminal sequences removed and anything that looks like a credential (values of `TOKEN`, `SECRET`, `KEY`, `PASSWORD` variables, bearer headers, provider key prefixes, JWTs, private keys, long mixed letter-digit runs) replaced by `[redacted]`. It is a best effort. `cat .env` prints a file whose values do not look like credentials, so the PM and the Operator receive them. Do not approve commands that print secrets. Output is shown to agents as untrusted data.

**Limits you must accept.** An approved command runs with your full authority. A process of the same user that outlives its run (`setsid`, `nohup`) can reach the control socket or read tokens from `/proc`. Nothing proves a human approved a command beyond the PM prompt: the PM agent is the approver, and a PM that approves without asking you has bypassed the control. Read what the PM shows you.

**Audit.** Every proposal, decision, run, result and expiry is a ledger event. `cstan op show <id>` prints a proposal with its run and output tail; the ledger keeps them (tables `operator_proposals` and `operator_runs`).

## Session grants

Approving one proposal can also allow the same thing again for a while, so the PM does not ask you for every repeat. The PM adds one of `--session exact`, `--session prefix="<words>"`, `--session timed` or `--session timed=session` to the approval: `cstan op decide <id> approve --hash <hash12> --session exact`. A grant has a kind (`exact`, `prefix` or `timed`); `cstan op grants` shows the kind and the time left.

- **Exact.** An identical command (same text, same kind) from the Operator runs with no new approval; the proposal records `auto_rule` `session:<grant-id>`. Any changed text needs approval again.
- **Prefix.** The grant stores the words verbatim; the PM notice, `cstan op show` and `cstan op grants` print them. A later command matches only when it starts with those whole words (`ls -l` matches `ls -l docs`, not `ls -la` and not `lsx -l`). The rest of the command goes through the same check as any command: if it has a metacharacter, a second line or any word from the denylist (`push`, `rm`, `-f`, ...), it needs approval. The controller refuses a prefix that contains a denylist word, that is empty, that is not a simple one-line command, or that is only `git` or `cstan` with no subcommand.
- **Timed.** `--session timed` auto-approves safe read-only commands for `timed_grant_default_minutes`; `--session timed=session` does so until the cap `timed_grant_max_minutes`, and also ends on Operator release, restart or revoke. Only a command that starts with an entry of `timed_safe_commands`, where every part of a `|`, `&&`, `||` or `;` chain is itself safe (any other shell syntax is refused), and whose path arguments are relative, contain no `..` and no dot-component (such as `.env` or `.git`) is covered; the run records `auto_rule` `session:<grant-id>`. A timed grant never covers a command with an always-approve word (`push`, `rm`, `-f`, ...), a destructive, unknown or unparseable command, or a restart: those need the PM as usual. Every run under it is still reported to the PM. The PM offers it as "Auto-approve safe (read-only) commands" in the approval picker, with the choices "For 1 hour" and "For this session (up to the configured cap)".
- **End.** A grant ends when the Operator agent is released or replaced, when the controller restarts, when `session_grant_max_minutes` has passed (the cap is measured with the system clock, because grants are stored), or when the PM or you run `cstan op revoke <grant-id>`. A command proposed after that needs approval. `cstan op grants` lists the grants in force. Each grant is created, used, revoked or expired as a ledger event.

**Accepted risk of the path rule.** The check that path arguments are safe is lexical only. It does not follow symlinks, so a tracked symlink that points outside the project or at a secret is not detected, and it only rejects dot-components, so a secret file with a non-dot name such as `config/secrets.json` (no leading dot) is not detected. A git helper configured in the project can also run project code. A timed grant accepts these risks; do not approve one in a project where they matter.

A prefix widens what a later command can do, up to the denylist. Read the prefix the PM shows you before you agree.

## Full auto

Full auto switches off every guard for a limited time. While it is on, each Operator proposal is approved at the moment it is proposed (`auto_rule` `full-auto`) and runs: no allowlist, no denylist, no PM decision. Pushes and deletes are included, and so are restarts (a restart still waits for the idle check and still needs the rollback target). Every run row and event is marked `full_auto`.

- `cstan op full-auto on [<minutes>] --asked-user "<what the user said>"` — the PM only. The minutes default to `full_auto_default_minutes` and may not exceed `full_auto_max_minutes`. The PM and the Operator agent get a notice when it goes on, off or expires.
- `cstan op full-auto off` — the PM or the operator CLI (you). It takes effect at once. An approved proposal that has not started when full auto goes off is not run: it ends, the PM is told, and the Operator must propose again.
- `cstan op full-auto status` — shows the minutes left. `cstan status --watch` and `cstan dash` show the time left and the grants in force.
- The operator CLI cannot switch full auto on: you switch it off, the PM switches it on after it asked you.
- Full auto is kept in the controller's memory only. After a restart (including an Operator restart) it is off, a startup event records that, and the restart notice says so. The time box uses a monotonic clock, so a change of the system clock cannot lengthen it.

**The risk, stated plainly.** Full auto removes every guard by your decision. A PM that turns it on without asking you has bypassed you, and the controller cannot tell: the `--asked-user` text is only recorded. What remains is the ledger audit (the text, the time, every proposal and run marked `full_auto`) and the time box. There is no denylist, no hard floor and no confirmation in full auto, and pushes and deletes are included. Leave `[operator]` out, or `enabled = false`, if you never want this.

## Restart and rollback

`cstan op propose --restart [--force] "<reason>"` asks the controller to restart itself so a new `cstan` binary takes effect: install the new binary over the running one (the installer does this), then propose the restart. There is one flag name: `--force` on `propose` (there is no `--force-restart` and no flag on `decide`); the hash covers it.

1. **Refused without a rollback target.** The controller keeps a known-good copy of the binary it is running in `.capstan/state/known-good/` (`cstan` plus `manifest.json` with `createdAt`, `controllerVersion`, `maxMigration` and an empty `deps`). It saves the copy 60 seconds after start, and only when the binary on disk still equals the build the controller loaded: a build installed in that window never becomes known-good. A copy made by an earlier version holds `cstan-daemon` instead of `cstan`; it is accepted as the rollback target, and the next successful start saves a new copy as `cstan`. Until a copy exists, `op propose --restart` is refused with `no_known_good`, and the check is repeated when the restart runs. A restart never starts without a rollback target.
2. **Idle check.** Unless `--force` was proposed, the restart waits up to `restart_idle_wait_seconds` until no spawn, release or replace is in flight, no review is `started`, no integration is running or merged but unsettled and no delivery is sent but unacknowledged (Operator runs are not counted). Otherwise it fails with the list of what was busy and the PM is told. Re-propose with `--force` to restart anyway; the PM approves the hash that covers it.
3. **Detached helper.** The controller writes `.capstan/state/restart/<id>/plan.json`, starts the known-good binary as `cstan __restart-helper <plan>` detached in its own process group, and stops itself with the same graceful stop as the `shutdown` command. The helper waits for the socket and pid file to go (SIGTERM, then SIGKILL after 10 seconds, for a stuck old controller), copies the ledger and its `-wal` and `-shm` files into `restart/<id>/ledger.bak/`, starts the new controller and pings it over the control socket every second until `restart_health_timeout_seconds`. Success writes `result.json` with outcome `ok`.
4. **Rollback.** If the new controller exits or does not answer, the helper stops it, renames the binary to `<binary>.failed-<id>`, restores the known-good binary (copied beside it, then renamed into place, so it is never half-copied) and starts it again (at most twice). If the stored ledger schema is newer than `manifest.maxMigration`, the helper first restores the ledger from `ledger.bak`; writes made only by the failed new controller are lost, and the PM notice says `LEDGER RESTORED`. The result is `rolled_back` with the reason. If the old build does not start either, the result is `down` with the manual recovery steps. If the handoff fails, the result is `rolled_back` only when the old controller still answers ping; otherwise it is `down` and says the controller is not running. An unreadable operator key or a corrupt plan also ends as `down`, and the controller refuses a restart whose key file it cannot read.
5. **After the restart.** At every controller start the controller first ingests any `result.json`, tells the PM and the Operator and finishes the run (`finished`, `failed` for a rollback, `error` for `down`); only then does it abandon runs that were running when the last controller stopped. A restart whose helper is still working is left running and its result is ingested when it appears; a restart with no result and no live helper becomes `abandoned`. Agents' panes are re-adopted by the normal startup.

**Manual recovery** when the outcome is `down` or the controller is not running after a restart (the PM notice and `result.json` name the paths). From the project root:

```sh
cp .capstan/state/known-good/cstan <binary>.new && chmod 755 <binary>.new && mv <binary>.new <binary>   # or inspect <binary>.failed-<id> first
# if the controller refuses the ledger: copy .capstan/state/restart/<id>/ledger.bak/* over .capstan/state/ (keep the file names)
cstan start
```

The known-good copy is not refreshed during a restart. The helper never deletes anything it did not create; `restart/<id>/`, `ledger.bak/` and `<binary>.failed-<id>` stay until you remove them.

## Liveness: an agent whose CLI has exited

The controller records an agent lost in two ways. The first is a pane that is gone (Herdr no longer finds it for three observations in a row). The second, described here, is a pane that stays open while the CLI inside it has exited, so the shell sits at its prompt.

**Detection.** For each active agent that has a pane, the controller asks Herdr for the pane's shell pid (`pane process-info`, already used for the activity check) and looks at the operating system's process table. The shell having at least one live child process of any kind means the CLI is running; the command is not looked at, so any CLI a role's host config allows is covered, and no screen text is used. On Linux the check follows the kernel's `children` lists and falls back to the whole process table (and to `ps` elsewhere), as the activity check does.

**Guards.** An agent is recorded lost with the reason `process_exited` only when `confirm_checks` samples in a row find no child process and they span at least two `check_seconds`. Any of these starts the count again: a live child, a Herdr observation of `working`, a failed Herdr call or an unreadable shell pid or process table (an unknown sample counts for nothing, in either direction). An agent is never judged within `start_grace_seconds` of the moment the controller first saw it or its new generation. When Herdr reports every pane missing at once (a Herdr restart) the check is skipped, as the pane-gone rule is. The PM gets the usual Agent lost notice, once. A pane that still answers does not clear the loss; only a live process does.

**Settings.** The `[liveness]` table (defaults): `enabled = true`, `check_seconds = 30`, `confirm_checks = 3`, `start_grace_seconds = 120`. A dead CLI is recorded lost about 90 to 120 seconds after it exits. Set `enabled = false` to turn the process check off; pane-gone detection is not affected.

**What happens next.** A lost Supervisor is released by the controller (the pane is closed and the agent ends) and the next supervision tick starts a new one; if the Supervisor role is at its `max_active`, the refusal is logged once as `supervisor_spawn_refused` and the start is retried after a minute. A lost Developer or Verifier is not released automatically: the PM decides with `cstan replace` or `cstan release`.

**Limits.** A role whose host command replaces the shell (exec) closes its pane when the CLI exits; that is the pane-gone case and is covered by it. A CLI wrapper that daemonizes and exits leaves no child process, so it looks dead; set `liveness.enabled = false` for such a setup.
