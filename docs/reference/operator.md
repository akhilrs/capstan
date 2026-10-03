# Operator reference

Back to the [README](../../README.md).

An optional **Operator** agent lets the PM have a shell command run, or the controller restarted, without you at the keyboard. The Operator only proposes; the controller runs. Nothing about the Operator exists while `[operator]` is absent or `enabled = false`: no role, no prompt text, no `op` command (it answers `not_configured`), no restart code loaded, no known-good snapshot taken, no `restart/` directory.

**Model.** The Operator is a Developer-kind role named by `[operator] role`. It runs `cstan op propose "<command>" "<reason>"` (the command first, then the reason; or `cstan op propose --restart [--force] "<reason>"`). The controller stores the proposal with the exact command text and a hash of kind, text and force flag, tells the PM, and runs approved proposals one at a time as the project user, in the project root. The runner passes the approved command text to `sh -c` (`src/command-runner.ts`), so the shell interprets that text (quoting, expansion, pipes). The text is exactly what was proposed and approved; it is never assembled from other fields. An active PM agent approves with `cstan op decide <id> approve --hash <hash12>`; the hash must match the stored proposal. The operator CLI (you, with `.capstan/operator.key`) can only `deny`, `cancel`, `show`, `revoke` a grant and switch full auto `off`.

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
| `full_auto_default_minutes`               | `30`           | Full auto duration when the PM gives none (at most the maximum).  |
| `full_auto_max_minutes`                   | `120`          | Longest full auto period (at most 480).                           |

**Auto allowlist.** Only exact commands that match `OPERATOR_AUTO_ALLOWLIST` in `src/operator-policy.ts` and are listed in `auto_approve` (or match an `auto_approve_prefix`) run without the PM: `ls` with `-l -a -la -h` and plain path arguments, `pwd`, `whoami`, `date`, `uname -a`, `df -h`, `cstan ping`, `cstan status`, `git rev-parse` (`--abbrev-ref`, `--short`, `HEAD`) and `git ls-files`. `git status`, `git diff` and `git show` are deliberately not on it: they can run project-configured helpers (`diff.external`, textconv, pagers), so they can execute project code. A restart is never auto-approved, even if you list it.

**Denylist.** A second layer, not the guard: a command containing words such as `push`, `rm`, `reset`, `checkout`, `merge`, `sudo`, `curl`, `ssh`, `sh`, `bash`, `eval`, `xargs`, `find`, `npm`, `node`, `python`, `make`, `docker`, or the short options `-f -d -D -x -r -R -o -O -e -c`, always needs the PM, whatever the rules say. The full list is `OPERATOR_ALWAYS_APPROVAL` in `src/operator-policy.ts`.

**PM approval.** The PM sees the exact command, the reason and the hash, shows you the command and decides. Approval is bound to the text, the kind and the force flag: a changed text has another hash. An approved proposal runs once; no approval is taken while a run is in progress. Command text and reason are printable ASCII only.

**Working directory and environment.** Commands run in the project root. The child gets no `CAPSTAN_` variable and no `cstan` wrapper on its `PATH`.

**Output sanitisation, and its limits.** The output tail has terminal sequences removed and anything that looks like a credential (values of `TOKEN`, `SECRET`, `KEY`, `PASSWORD` variables, bearer headers, provider key prefixes, JWTs, private keys, long mixed letter-digit runs) replaced by `[redacted]`. It is a best effort. `cat .env` prints a file whose values do not look like credentials, so the PM and the Operator receive them. Do not approve commands that print secrets. Output is shown to agents as untrusted data.

**Limits you must accept.** An approved command runs with your full authority. A process of the same user that outlives its run (`setsid`, `nohup`) can reach the control socket or read tokens from `/proc`. Nothing proves a human approved a command beyond the PM prompt: the PM agent is the approver, and a PM that approves without asking you has bypassed the control. Read what the PM shows you.

**Audit.** Every proposal, decision, run, result and expiry is a ledger event. `cstan op show <id>` prints a proposal with its run and output tail; the ledger keeps them (tables `operator_proposals` and `operator_runs`).

## Session grants

Approving one proposal can also allow the same thing again for a while, so the PM does not ask you for every repeat. The PM adds `--session exact` or `--session prefix="<words>"` to the approval: `cstan op decide <id> approve --hash <hash12> --session exact`.

- **Exact.** An identical command (same text, same kind) from the Operator runs with no new approval; the proposal records `auto_rule` `session:<grant-id>`. Any changed text needs approval again.
- **Prefix.** The grant stores the words verbatim; the PM notice, `cstan op show` and `cstan op grants` print them. A later command matches only when it starts with those whole words (`ls -l` matches `ls -l docs`, not `ls -la` and not `lsx -l`). The rest of the command goes through the same check as any command: if it has a metacharacter, a second line or any word from the denylist (`push`, `rm`, `-f`, ...), it needs approval. The controller refuses a prefix that contains a denylist word, that is empty, that is not a simple one-line command, or that is only `git` or `cstan` with no subcommand.
- **End.** A grant ends when the Operator agent is released or replaced, when the controller restarts, when `session_grant_max_minutes` has passed (the cap is measured with the system clock, because grants are stored), or when the PM or you run `cstan op revoke <grant-id>`. A command proposed after that needs approval. `cstan op grants` lists the grants in force. Each grant is created, used, revoked or expired as a ledger event.

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

`cstan op propose --restart [--force] "<reason>"` asks the controller to restart itself so a new build of `dist/` takes effect. There is one flag name: `--force` on `propose` (there is no `--force-restart` and no flag on `decide`); the hash covers it.

1. **Refused without a rollback target.** The controller keeps a known-good copy of the build it is running in `.capstan/state/known-good/` (`dist/` plus `manifest.json` with `createdAt`, `controllerVersion`, `maxMigration` and the sha256 of `package.json`, `package-lock.json` and `node_modules/.package-lock.json`). It saves the copy 60 seconds after start, and only when `dist/` on disk still equals the build the controller loaded: a build run in that window never becomes known-good. Until the copy exists, `op propose --restart` is refused with `no_known_good`, and the check is repeated when the restart runs. A restart never starts without a rollback target. Restart proposals are also refused with `no_schema_probe` when Node has no `node:sqlite` (the helper reads the ledger schema with it; Node 22.5 or newer).
2. **Idle check.** Unless `--force` was proposed, the restart waits up to `restart_idle_wait_seconds` until no spawn, release or replace is in flight, no review is `started`, no integration is running or merged but unsettled and no delivery is sent but unacknowledged (Operator runs are not counted). Otherwise it fails with the list of what was busy and the PM is told. Re-propose with `--force` to restart anyway; the PM approves the hash that covers it.
3. **Detached helper.** The controller copies `restart-helper.js` to `.capstan/state/restart/<id>/helper.mjs`, writes `plan.json`, starts the helper detached in its own process group, and stops itself with the same graceful stop as the `shutdown` command. The helper waits for the socket and pid file to go (SIGTERM, then SIGKILL after 10 seconds, for a stuck old controller), copies the ledger and its `-wal` and `-shm` files into `restart/<id>/ledger.bak/`, starts the new controller and pings it over the control socket every second until `restart_health_timeout_seconds`. Success writes `result.json` with outcome `ok`.
4. **Rollback.** If the new controller exits or does not answer, the helper stops it, renames `dist/` to `dist.failed-<id>`, restores `known-good/dist` (copied beside it, then renamed into place, so it is never half-copied) and starts it again (at most twice). If the stored ledger schema is newer than `manifest.maxMigration`, the helper first restores the ledger from `ledger.bak`; writes made only by the failed new controller are lost, and the PM notice says `LEDGER RESTORED`. The result is `rolled_back` with the reason. If the old build does not start either, the result is `down` with the manual recovery steps. If the handoff fails, the result is `rolled_back` only when the old controller still answers ping; otherwise it is `down` and says the controller is not running. An unreadable operator key or a corrupt plan also ends as `down`, and the controller refuses a restart whose key file it cannot read.
5. **Dependency changes.** If `package.json`, `package-lock.json` or `node_modules/.package-lock.json` differ from the manifest when you propose, the PM notice says `dependencies changed since the known-good build: rollback may not start`. Rollback is still tried; a failure ends as `down` and names the change.
6. **After the restart.** At every controller start the controller first ingests any `result.json`, tells the PM and the Operator and finishes the run (`finished`, `failed` for a rollback, `error` for `down`); only then does it abandon runs that were running when the last controller stopped. A restart whose helper is still working is left running and its result is ingested when it appears; a restart with no result and no live helper becomes `abandoned`. Agents' panes are re-adopted by the normal startup (`adoptAll`).

**Manual recovery** when the outcome is `down` or the controller is not running after a restart (the PM notice and `result.json` name the paths). From the project root:

```sh
rm -rf dist && cp -a .capstan/state/known-good/dist dist   # or inspect dist.failed-<id> first
# if the controller refuses the ledger: copy .capstan/state/restart/<id>/ledger.bak/* over .capstan/state/ (keep the file names)
cstan start
```

The known-good copy is not refreshed during a restart. The helper never deletes anything it did not create; `restart/<id>/`, `ledger.bak/` and `dist.failed-<id>` stay until you remove them.
