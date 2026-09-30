# Capstan MVP Plan v2

## 1. Purpose and status

**Status:** proposed with DEC-005 (PM-20). It becomes the plan of record when the PR that adds it is merged by the operator. `MVP_PLAN.md` (M0 to M3) is superseded in part: its Docker, network, egress and OMP-bridge material is history; its controller-core, ledger and recovery material is reused.

**Product:** Capstan; **CLI:** `cstan`. The operator runs `cstan` in a project. It opens the configured agent host in Herdr. The first agent is the PM: the operator types the requirement to it and talks with it. The PM asks Capstan to spawn other agents (developer, reviewer, later designer, tester and a Supervisor). Agents are interactive and persistent. The task ends when the work is integrated, independently verified and accepted.

**Evidence base.** The PM-19 spike (`docs/spike-herdr-agents.md`, experiments E1 to E6 with Claude Code in an isolated Herdr session, each run once) and an external plan review of the redesign (findings R-1 to R-9, section 14). Where this plan relies on something the spike did not test, it says so (section 12).

**Working names.** Command names (`cstan start`, `send`, `inbox`, `wait`, `ack`, `report`, `ask`, `request-review`, `finding`, `status`, `pm restart`) and the config file `capstan.toml` are working names.

## 2. Decisions taken by the operator

No Docker, no per-role networking or firewall. One trusted single-user VM. Isolation is git worktrees created by `herdr worktree create`. Containment is process-group only. Herdr stays. Agents are interactive and persistent. The PM is the interactive main agent and asks Capstan, never Herdr directly, to spawn and message agents. A separate always-on Supervisor. Claude Code first, Codex and OMP later by configuration. Per-role settings in `capstan.toml`. Build a thin vertical slice first: PM, one developer, one reviewer. The Supervisor comes after the slice. See DEC-005 for what this supersedes and for the trust model.

## 3. Hypotheses kept from the MVP

| ID | Hypothesis | Status in v2 |
| --- | --- | --- |
| H1 | Separate agents can collaborate without one shared conversation | kept; tested from Stage 2 |
| H2 | Durable state prevents forgotten ownership and next actions | kept; Stage 6 |
| H3 | Independent supervision can break an unproductive loop | kept; Stage 5; independence is by convention only (same account and VM) |
| H4 | Failure recovery does not introduce duplicate execution | kept; Stages 2 and 6 |
| H5 | Independent verification prevents premature completion | kept; Stages 3 and 4 |
| H6 | Coordination cost is measurable and potentially worthwhile | kept; comparison against a baseline is after the slice |

## 4. Architecture

```
 operator ── cstan start ──► reads capstan.toml (roles: host, model, permissions)
                              │
        ┌─────────────────────▼───────────────────────────────────────────────┐
        │ CONTROLLER DAEMON (separate process, single-instance lock)          │
        │  SQLite ledger: work, assignments, generations, messages, receipts, │
        │                 candidates, findings, events                        │
        │  Herdr adapter: the only process that sends input to, starts,       │
        │                 stops or closes agents in Herdr                     │
        │  message queue: one writer and one FIFO per recipient               │
        │  control socket (Unix, 0600): `cstan` commands from agents/operator │
        └───────┬───────────────┬───────────────┬─────────────────┬───────────┘
                │ herdr          │ herdr          │ herdr            │
                ▼                ▼                ▼                  ▼
      pane: PM (Claude Code)  pane: developer  pane: reviewer   pane: supervisor
      operator types here     worktree g1      read-only        (after the slice)
                │                │                │
                └── agents report facts by running `cstan ...` (receipts) ──┘
```

Two channels:

1. **Conversation** goes through the controller's Herdr adapter: guidance, questions, answers.
2. **Facts** are `cstan` commands run inside the agent's pane: `report`, `ask`, `ack`, `inbox`, `wait`, `request-review`, `finding`. Completion is never inferred from the terminal or from Herdr's state. It is a receipt.

The controller runs with no PM alive. If the daemon dies, `cstan start` (or any `cstan` command) restarts it and reconciles. An automatic supervisor for the daemon, for example a systemd user unit, is a later item.

## 5. Messaging

`cstan send` is the only path for agent-to-agent text. The controller keeps one queue and one writer per worker pane and one strict FIFO order per recipient.

**Principle for reading panes.** Reading the screen may block or defer an action, or be shown as evidence. It never causes a state change or an automatic resend. Exactly two cases let a screen read gate an action, both logged (DEC-005): the pre-send empty-input check for worker panes, and the startup trust dialog.

**Workers (push).** A message is delivered only when the agent's Herdr state is `idle` or `done` and a read of the pane shows an empty input line. Otherwise it is `deferred`. It never sends while the agent is `working` or `blocked`. If the input line is still not empty after the maximum deferral, the controller reads and logs the text, notifies the operator, clears the line with `ctrl+u`, and then sends the message.

**The PM (pull; nothing is ever typed into the PM pane).** The operator may be typing in the PM pane, and text already in an input line merges with a later prompt (spike E4). So:

- The PM's role prompt tells it to run `cstan inbox` at the start of every turn and, only while it is actively waiting on delegated work, to call `cstan wait --timeout 90` in a loop.
- `capstan.toml` has `wait_timeout_seconds` (default 90) and `host_shell_timeout_seconds` per host, and rejects a configuration where the first is not below the second. The role prompt states the timeout. The plan reviewer reported about two minutes as Claude Code's default shell-command timeout; the spike did not test this, and Stage 2 checks it. A wait therefore ends by itself and the PM decides whether to call it again.
- `inbox` and `wait` print message text and move the message to `sent`. They do not ack. The PM acks each message with `cstan ack <id>`, as workers do. Whether the operator was notified is a separate flag (`notified_at`), not a state.
- `inbox` and `wait` also re-print the PM's own messages that are `sent` or `unacked` and not yet acked. That is a re-read, not a resend, and lets the PM ack after an interruption without the operator's help. Acking an `unacked` message moves it to `acked_late`.
- Strict FIFO applies, so `inbox` returns the oldest unresolved message and the next message becomes eligible when it is acked: one message per ack. This is a known throughput limit of the slice; batching is a later item. A gate case checks several queued worker reports (section 11, Stage 2).
- The operator's own `cstan inbox` is read-only (peek) by default, so it cannot move PM messages to `sent` unseen.
- An interrupted, timed-out or backgrounded `wait` is routine, not a residual case: the operator presses Esc to talk to the PM (the host is reported to queue typed input until the call returns, which the spike did not test and Stage 2 checks) and the model may send the command to the background. Because the ack is explicit, a message printed by such a call stays `sent`, is never lost, and becomes `unacked` after the timeout. While the PM has a registered `wait` in progress, including a backgrounded one until it returns, the controller exempts it from stall timers, because Herdr shows the PM as `working` during the wait. Each `wait` is a tool call; the context cost per loop is measured in Stage 2.
- PM ack timeout: default 10 minutes. The timer does not run while the PM's Herdr state is `working` outside a registered `wait`, so a long PM turn does not send every printed message to `unacked`. Time spent inside a registered `wait` does count, because a PM that keeps looping in `wait` without acking is ignoring the messages it was shown, and Herdr shows the PM as `working` throughout every wait. Stall timers still skip registered waits (a waiting PM is not stalled); the ack and notification timers do not.
- Notification: if a PM message stays `queued`, `sent` or `unacked` for longer than `pm_notify_after` (default 5 minutes, not counting time while the PM's Herdr state is `working` outside a registered `wait`, like the ack timer; the deferral limit that applies to worker messages does not apply to the PM), the controller raises a `herdr notification` to the operator and repeats it every N minutes (default 10) while the queue is blocked. If `herdr notification` does not work, a pane the controller owns runs `cstan status --watch` (typing into an owned pane is safe) and rings the terminal bell. There is no automatic skip: a blocked PM queue waits for the operator.
- Optional enhancement, verified in Stage 2 and dropped if it fails: a `UserPromptSubmit` hook for the PM that runs `cstan inbox --peek` (a one-line count, marks nothing). It needs the operator's hooks excluded (`--setting-sources project,local`, with the Capstan hook added through `--settings`, because `--settings` alone merges with `~/.claude`), and that may also drop Herdr's own Claude hook. No `Stop` hook that adds context is used: it would force the PM to continue, which is controller-driven input.

**Message states.** `queued`, `deferred` (agent busy or blocked, or input line not empty; has a maximum deferral, after which a non-empty input line is handled as in the Workers rule above: the text is read and logged, the operator is notified, the line is cleared with `ctrl+u` and the message is then sent; a busy or blocked agent makes it `expired`), `sent`, `acked`, `acked_late` (an ack arriving after `unacked`), `unacked` (no ack in the timeout), `expired`, `cancelled` (the recipient's generation was replaced or the task ended), `failed`.

**Ordering.** Every unresolved state (`deferred`, `sent`, `unacked`, `expired`, `failed`) blocks later messages to the same recipient. Resolved states (`acked`, `acked_late`, `cancelled`) do not. Who resolves a blocked message: for a worker, the PM or the operator; for the PM, only the operator, because the PM cannot resolve a message addressed to itself. Resolution is a recorded decision: `retry` (an explicit resend), `skip` or `cancel`.

**No automatic resend, ever.** A resend can run an instruction twice, and pane reading is not a reliable trigger (E3, E4). Reading the pane after `unacked` is evidence shown to the PM or operator.

**Rules fixed by the controller core (PM-22, migration 0015).** These make the paragraphs above exact; nothing here relaxes them.

- **Timers are computed, not stored.** The controller records each change of an agent's observed Herdr state and each registered `wait` (open until it returns; the daemon closes it when the `cstan wait` connection drops). Timers are evaluated from those facts and the clock. For the PM ack and notification timers, time counts while the PM is not `working` and all time inside a registered wait counts. Stall time counts only while the agent is `working` outside every wait, measured from its last own command. An agent with no observation counts as not `working`.
- **Worker ack timeout.** A `sent` worker message becomes `unacked` after `worker_ack_timeout_seconds` (default 600) of wall-clock time, in `capstan.toml` under `[timers]`.
- **Resolutions.** `retry` returns a `deferred`, `sent`, `unacked`, `expired` or `failed` message to `queued` and restarts its notification clock; `skip` and `cancel` move it to `cancelled` and also apply to a `queued` message. A message addressed to the PM is resolved only by the operator. Every resolution is a recorded row.
- **Notification.** Only the head of a PM queue notifies; later messages are blocked behind it.
- **Record before send.** The driver records `recordSent` (and, for a non-empty input line, records the text with `recordInputClear` first) before it performs the physical send. A crash between the record and the send leaves the message `sent`; nobody acks it, it becomes `unacked`, and the PM or operator decides. The reverse order could send a message twice after a crash and is not allowed.
- **Generations.** An agent's actor changes with each generation: `replaceAgentGeneration` revokes the old actor, issues a new one, closes the agent's waits and cancels every open message of the old generation in one mutation. A token of an earlier generation fails in authentication.
- **Message bodies.** A body is plain text of 1 to 16384 bytes. Line feed and tab are allowed; carriage returns, escape sequences and other control, format and line-separator characters are refused, because the driver types the body into a pane and any actor with `message:send` could otherwise inject keystrokes. Zero-width joiners stay allowed so joined emoji sequences and non-Latin scripts work; Unicode tag characters (which can hide text and are the basis of subdivision flags) and lone surrogates are refused. The audit trail holds only the body's hash.
- **Rejections.** An illegal transition, an ack by a non-recipient and a delivery of a message that is not the queue head are written to `message_rejections` and raised as an error; the same call replayed with the same idempotency key raises the same error.

Each message carries an id and asks for `cstan ack <id>`. The ack shows receipt, not understanding, and is forgeable (E5). A nonce adds nothing against a same-user forger and is left out of the slice.

## 6. Facts are verified, not trusted

An environment-variable credential only labels a reporter. Any same-user process can read it and forge acks and reports (E5). The controller narrows what a forger can do; it does not close it:

- A reported commit must exist on the branch recorded for that agent's current generation; the generation must be current; the state transition must be legal; acceptance needs an independent reviewer receipt bound to that exact commit.
- Not closed: a process holding a stolen token that also has a valid commit on that branch is accepted. Any same-user process can commit into any worktree. This is a known hole (DEC-005), and the Stage 3 gate records it as a documented limit, not as a pass.
- Tokens are per agent and per generation and are never printed. The ledger records the claimed identity together with the evidence that was checked.

**Operator authority and the control channel.** `cstan` reaches the daemon over a Unix socket in the state directory (mode 0600). Any process of the same user can use it. Agent commands (`report`, `ask`, `ack`, `inbox`, `wait`, `request-review`, `finding`, and the read-only `status`) need the agent's token. Operator commands (`assign`, `cancel`, `send` as the operator, `pm restart`, resolving a blocked message) need an operator credential that is not put in any agent's environment. Each role's profile carries host deny rules for the operator commands. The read-only commands are usable with either credential: an agent, including the PM's optional hook, uses its token for `status` and `inbox --peek`; the operator and the controller-owned `cstan status --watch` pane use the operator credential in read-only mode. Both controls are only as strong as the same-user boundary: an agent that reads the operator credential file, or ignores its deny rules, can forge the operator's authority. This is a listed residual risk, not a solved problem.

## 7. Herdr state is a hint

With the installed hook, the idle and blocked states that were explained came from screen-detection rules with a remotely updated manifest (E3). Herdr state decides when to send and spots `blocked`. Stalls (`working` with no receipt and no registered wait for a configured time) are detected by controller timers from Stage 2, not by the Supervisor, and reported to the PM and operator. Nothing that matters relies on Herdr state alone. Pinning or disabling manifest auto-update is an open item; a Claude Code, Herdr or manifest update is a revisit trigger (DEC-005).

## 8. Permissions and how agents learn the commands

- **Per-role profile** in `capstan.toml`: host, model, permission mode, allow and deny rules, and for workers `-- --settings '{"disableAllHooks":true}'` so the operator's own hooks (claw8) do not interfere (verified in the spike's additional finding on global Claude configuration). Turning hooks off also drops guards the operator may want, and the rest of `~/.claude` was not examined. Auto permission mode is off for spawned agents unless the profile says so; an agent that inherits auto mode runs commands without asking.
- **`cstan` is allow-listed per role.** In default permission mode a command outside the allow rules prompts (E3). The agent learns the commands from a role prompt passed with `--append-system-prompt`. Both are verified in Stage 2, before anything depends on `ack`.
- **Confinement.** `--add-dir` adds directories; it does not limit an agent to one. Confinement to the worktree relies on the working directory plus per-role allow and deny rules for writes, and holds only as far as the host enforces them. This plan claims no more.
- **Prompts.** A permission prompt makes the agent `blocked`. The controller notifies the operator. The PM never answers permission prompts. Nothing presses Enter on a prompt without reading it: the trust dialog defaults to "No, exit" and the permission prompt defaults to "Yes" (E1, E3).
- **Trust dialog exception** (DEC-005): the controller may answer the startup trust dialog for a worktree path it has just created, after reading that the dialog names exactly that path and choosing the option by its text. This is a screen-driven keypress, so it is scraping and it is logged. Pre-trusting through the host's configuration is preferred if it works.

## 9. Recovery, roles and persistent agents

- **PM failure and takeover.** The controller runs without the PM. `cstan pm restart` (Stage 2, with the slice) starts a new PM with a summary rebuilt from the ledger: objective, decisions, open work, unacknowledged messages. The operator can always act through `cstan` directly (`status`, `assign`, `cancel`, `send`, `inbox`).
- **Persistent agents.** An agent lives until the task ends. Its assignment holds many rounds (an instruction, then a report). A replacement happens only on failure and is seeded from the ledger. Automatic recycling for heavy context is out of the slice.
- **Roles from configuration.** `capstan.toml` gives each role a free name and one of four kinds: `PM`, `Developer`, `Verifier` or `Supervisor`. The kind decides the capabilities. `src/controller/types.ts` fixes six roles (`operator`, `controller` and the four kinds), `src/controller/auth.ts` checks that list, six tables carry it in CHECK constraints, and `src/controller/core.ts` hard-codes role names (the literal `Verifier` appears 37 times, including in acceptance and final-verification logic), so open role names are not possible in this slice. Migration 0014 (PM-21) adds one table, `role_definitions` (role name, kind, host, config hash, state), filled by `cstan config sync` through an audited controller mutation. A seat created from a role stores the role name in `seats.name` and the kind in `seats.role`; work items keep `required_role` as the kind, so two roles of one kind are told apart by their seat. `auth.ts` and every existing table are unchanged. A role whose capability set matches no kind needs a per-actor capability grant or a later migration that rebuilds the CHECK constraints. The `agents`, `messages` and `rounds` tables come with the tickets that use them. The slice defaults are `pm`, `developer` and `reviewer`; `supervisor` is added in Stage 5.
- **Review evidence contract.** Review acceptance reuses DEC-004's candidate-evidence rules: verifier evidence carries a pass value, a non-empty artifact reference and an exact criterion string from the accepted input snapshot, bound to the exact candidate commit. How the PM supplies the acceptance criteria through `cstan` is a Stage 3 to 4 design item.

## 10. Worktrees, branches, review and integration

- **Branches.** Named `cap/<task>/<agent>-g<generation>`. A branch belongs to one agent generation. After a replacement, the new generation gets a new branch created from the last accepted candidate, or from the old branch tip only if the ledger records that tip as the resume point. "The agent's branch" always means the branch recorded in the ledger for the current generation.
- **Reviewer.** `cstan request-review` makes the controller create a worktree on a review branch at the given commit (`herdr worktree create --base <commit>`), start a reviewer there with a read-only profile, and remove the worktree and branch after the report. Findings come back through `cstan report`. The reviewer is never the author's session (controller-enforced). One review round per request.
- **Integration.** A controller operation in a separate integration worktree. The base is the main branch head at the time of integration, recorded in the ledger; the merge order is the order the PM recorded in the ledger. A conflict blocks and is reported to the PM; the controller never resolves it with a model. A developer may be assigned to resolve it as a new candidate. The integrated commit is re-verified by a reviewer before acceptance.
- **Cleanup.** Worktrees are removed at task end or on cancel (`herdr worktree remove`). Branches are kept until the PM or operator confirms deletion.
- **Supervisor (after the slice).** Always on, read-only by permission profile, watching through `herdr agent list/read/wait` and controller digests, reporting through `cstan finding`. Only the controller sends input to, starts, stops or closes agents; read-only inspection commands (`agent list`, `agent read`, `agent wait`) are allowed for the Supervisor and for diagnosis. The controller routes findings through the same message path. Independence is by convention only.

## 11. Build order (each stage has an exit gate with a stated oracle)

| Stage | Work | Exit gate |
| --- | --- | --- |
| 0 | Spike PM-19 | Done and externally reviewed. |
| 1 | This plan and DEC-005 (PM-20) | Externally reviewed and merged. |
| 2 | Message path, PM entry and daemon lifecycle: `capstan.toml`, `cstan start`, the daemon with a single-instance lock, `cstan pm restart`, the Herdr adapter, a minimal spawn of one developer worker (worktree, agent start and role prompt), `cstan send`, `inbox`, `wait` and `ack`, the message state machine, `cstan` allow-listed and taught by role prompt, controller stall timers. Also verified here because the gate depends on them: that `cstan wait` and `inbox` work as the PM's pull path, that `herdr notification` works (with its fallback), and whether a new worktree path can be pre-trusted in the host's configuration. | The Stage 2 gate, listed below the table. |
| 3 | Facts: `cstan report`, a branch per generation, controller-side verification of reported commits (the minimal spawn is built in Stage 2) | (a) a forged report for a commit that is not on that agent's branch, using a stolen token from another process, is rejected; (b) a forged report with the stolen token and a valid commit on that branch IS accepted, and the ledger shows the claimed identity and the evidence: this is recorded as the known hole, not as a pass. |
| 4 | Reviewer and integration | Independent review, findings, fix and re-review, with the reviewer never the author's session (oracle: session ids in the ledger); an integration conflict blocks and is reported; worktrees and review branches are removed afterwards. |
| 5 | Supervisor and findings routing | A developer is made to repeat the same failing tool call several times (for example a test that always fails with the same message). Oracle: a `finding` exists that cites the repeated outputs as evidence, it is delivered to the developer and acked, and a resolution check is recorded; or, if the developer does not recover within two correction attempts, an escalation to the operator is recorded. The limit of two corrective interventions for the same unresolved issue is carried over from `MVP_PLAN.md` (section 8, recovery defaults) and is already enforced by the controller (the test `finding correction budget permits two attempts and rejects a third` in `test/controller.test.ts`); a third attempt is not made. |
| 6 | Recovery: agent replacement, controller restart and crash; failure-injection harness rewritten for pane and process failures (PM-13 rescoped) | No duplicate execution (oracle: the agent is instructed to append one line to a file per instruction id; after a controller crash and an agent replacement the file has exactly one line per instruction id, or exactly two for an instruction whose resend was explicitly recorded) and no accepted stale report. |
| 7 | Codex and OMP adapters; removal of the Docker code and M1 scripts | Each host passes the Stage 2 gate; nothing Docker-related remains in the build. |


### Stage 2 gate

Run on real Claude Code sessions in an isolated Herdr session. Every case names its oracle.

- (a) 10 consecutive PM to developer rounds with every message answered exactly once (oracle: one answer per message id in the transcript, and `acked` for each in the message log).
- (b) with text already in a worker's input line the message is deferred, then delivered without merging (oracle: received text equals sent text); or, after the maximum deferral, the operator is notified, the text is logged, the line is cleared and the message is then sent (oracle: the log holds the text and the notification, and the received text equals the sent text).
- (c) a message queued while the agent is `working` is not sent until `idle` or `done` (oracle: send timestamps against recorded Herdr state history).
- (d) killing the developer's pane mid-round leaves the message `unacked` or `cancelled`, never silently lost, with no resend (oracle: message log).
- (e) `pm restart` writes a summary for the new PM from the ledger, and the new PM can list the open work: the open work items in the recorded summary equal a direct ledger query, and the new PM's answer to "list the open work" in its transcript matches that list.
- (f) the PM pull path: with the operator typing in the PM pane nothing is ever typed into it (oracle: pane input history shows only the operator's keys); a message reaches the PM through `inbox` and `wait`, moves to `sent`, and reaches `acked` only by an explicit `cstan ack`; operator typing during a `wait`, an Esc interrupt of the `wait`, a `wait` that hits the shell-command timeout and a `wait` sent to the background each leave printed messages `sent` and later `unacked` if never acked, never `acked` and never lost (oracle: message log after each case); several queued worker reports are delivered one per ack in order with none dropped; the PM is not flagged by stall timers during a registered `wait`, including a backgrounded one (oracle: finding log); a PM that loops in `wait` without acking reaches `unacked` and triggers a notification, while the ack and notification timers do not run when the PM works outside a `wait` (oracle: message log, notification record and timer log); a PM message left unacknowledged past `pm_notify_after` raises a `herdr notification`, repeated at the configured interval, and the fallback pane plus bell works with the notification disabled; the per-loop context cost of `wait` is measured and recorded; optional item: with `--setting-sources project,local` the operator's hooks do not run in the PM, the Capstan hook does, and the PM's Herdr state still works.
- (g) a message that expires or goes unacked blocks the next message to that recipient until a recorded resolution (oracle: message log ordering).
- (h) a new worktree path either starts without the trust dialog (pre-trusted; oracle: a pane read before the first prompt shows no dialog, and the recorded host configuration entry for that path exists) or the dialog is handled by the logged exception in section 8 (oracle: the dialog log shows the path read, the option chosen and each keypress).

## 12. Not verified

Stated so nobody builds on these as facts. Each is tested in the stage shown or deferred; a fallback is named only where one exists.

| Item | Tested in | Fallback |
| --- | --- | --- |
| `cstan wait` under Claude Code's shell-command behavior, including the reported two-minute timeout and Esc queuing typed input | Stage 2 | `cstan inbox` at the start of each turn plus the operator notification |
| `herdr notification` | Stage 2 | a controller-owned `cstan status --watch` pane plus a bell |
| Selective Claude Code hooks for the PM through `--setting-sources`, and whether Herdr's state still works with them | Stage 2 (optional) | none needed: the pull path does not use hooks |
| `cstan` allow-listing and the role prompt | Stage 2 | none: Stage 2 depends on it |
| Pre-trusting worktrees in the host's configuration | Stage 2 | the narrow logged trust-dialog exception (section 8) |
| Codex and OMP behavior | Stage 7 | none: Claude Code is the only host in the slice |
| The updated Herdr Claude hook and its effect on state accuracy | after Stage 2 | none: state is a hint |
| Long tasks and context growth; agent death and replacement | Stage 6 | none yet |
| Several agents interfering with each other | Stage 4 onward | none yet |
| Scrubbing the Herdr server's environment; pinning the detection manifest | not scheduled | none; recorded as residual risk in DEC-005 |
| Cost | before any long run | the operator's cap; none has been set |

## 13. Verification approach and cost

Deterministic tests for the message state machine, framing, fact verification and the migration. Real-host scenarios with Claude Code in an isolated named Herdr session (the spike's pattern), with evidence and negative results kept; no canned agent replies as evidence. Real-host runs spend the operator's Claude account, so each stage states its expected usage and the operator sets a cap before any long run. No cap has been set yet.

## 14. Traceability

**External plan review of the redesign (R-1 to R-9).**

| Finding | Where handled |
| --- | --- |
| R-1 impersonation is easy | Section 6 (facts verified, known hole); DEC-005 trust model |
| R-2 the ledger can be bypassed | Sections 4 and 5 (all sends through the controller); DEC-005 (unenforced, detection later) |
| R-3 permission answers by the PM are fragile | Section 8 (profiles, operator answers, PM never) |
| R-4 the PM is a single point of failure | Sections 4 and 9 (daemon, `pm restart`, operator commands) |
| R-5 typed messages can interleave or be lost | Section 5 (one writer, idle gating, explicit ack, pull for the PM) |
| R-6 the spike is too narrow | Done in PM-19, with the extra cases; Stage 2 gates add more |
| R-7 open roles need a data-model change | Section 9 (additive migration) |
| R-8 merging is missing | Section 10 (controller integration and re-verification) |
| R-9 independence and scope | Section 3 and section 10 (by convention; recycling and persistent reviewers cut from the slice) |

**Spike recommendations (`docs/spike-herdr-agents.md`).**

| Recommendation | Where handled |
| --- | --- |
| 1 Capstan sends every message; one writer; idle gating; ack | Section 5 (framing is id-only; the nonce was dropped because a same-user forger can copy it) |
| 2 Controller verifies reported facts | Section 6 |
| 3 Herdr state is a hint | Section 7 |
| 4 Never press Enter blindly; pre-trust; per-role permissions | Section 8 |
| 5 Controlled per-role config; hooks off through `--settings` | Section 8 |
| 6 Detect unrouted prompts by count | DEC-005: partial, not in the slice |
| 7 Prefer `herdr worktree create` | Section 10 |
