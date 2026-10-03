# How Capstan works

Concepts, the delivery flow, messaging, the worker lifecycle, supervision and status, as the code behaves. Back to the [README](../../README.md).

## Concepts

### Roles and seats

A role has a free name (for example `developer`, `designer`, `reviewer`, `tester`) and one of four **kinds**. The kind decides what the agent may do:

| Kind         | Purpose                                                                                                                                                            | What it may run                                                                                                   |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------- |
| `PM`         | Talks to you, plans, spawns and releases workers, sends tasks, reads reports, requests reviews, integrates, resolves delivery problems. Never edits project files. | `spawn`, `release`, `replace`, `send`, `inbox`, `wait`, `ack`, `request-review`, `integrate`, `observe`, `status` |
| `Developer`  | Implements or designs changes in its own worktree and branch (`developer` and `designer` in the starter config).                                                   | `ack`, `inbox`, `wait`, `send @pm`, `report`, `status`                                                            |
| `Verifier`   | Reviews one commit (`reviewer`) or runs tests (`tester`). A reviewer answers a review request once and is then ended by the controller.                            | `ack`, `inbox`, `wait`, `send @pm`, `report`, `review`, `status`                                                  |
| `Supervisor` | Watches the other agents and raises findings. Read and report only.                                                                                                | `status`, `observe`, `finding`, `inbox`, `wait`, `ack`                                                            |

`resolve` and `cancel` are registered as operator-only in `ROUTES` (`src/daemon.ts`), although the PM prompt (`src/prompts.ts`) tells the PM to run the `cstan resolve` a delivery notice names. Run them yourself from the project directory; an agent token is refused with `forbidden`.

A **seat** is one running agent of a role. Agents get ids such as `developer-1`; a replacement gets a new id and a higher generation (`g1`, `g2`, ...). The PM seat is created at `cstan start`; worker seats are created by `cstan spawn`. At most `limits.max_workers` workers (every agent except the PM) may be active at once.

### Controller and ledger

The **controller** is a long-running daemon (`cstan daemon`, started for you by `cstan start` and by any operator command). It holds a single-instance lock on the project, listens on a Unix socket in `.capstan/state/` and owns:

- **The ledger**, a SQLite database (`.capstan/state/controller.sqlite`, schema in `migrations/`) with agents, messages, reports, reviews, integrations, findings and the history of each.
- **All agent-to-agent messaging.** Only the controller types into worker panes, starts and closes agents in Herdr. The PM's pane is never typed into except for a one-line `Run cstan inbox` wake-up when it is idle.
- **Integration history.** An integration branch is the project's HEAD plus one squash commit (subject `<type>: <title>`, body listing the reports and agents); per-report commits stay only on worker branches. This describes integrations created after this change, not earlier ones.
- **Verification of reports.** Completion is never inferred from terminal text: it is recorded when an agent runs a `cstan` command, and the controller checks what it can (for a report: that the commit exists on that worker's branch after the branch started).

Agents authenticate to the daemon with a per-agent token (`CAPSTAN_TOKEN`) and socket path (`CAPSTAN_SOCKET`) in their environment. The operator (you) authenticates with the credential file `.capstan/operator.key`, which only works from the project directory.

## The delivery flow

The PM runs these steps on your behalf. You do the last one.

```text
 you ──task──▶ PM
                │ 1. cstan spawn <role>            controller: new pane + worktree + branch
                │ 2. cstan send <agent> "<task>"   controller queues, types it when worker idle
                ▼
             worker ── works, commits on its own branch (never pushes or merges)
                │ 3. cstan report <sha> "<summary>"
                ▼
        controller verifies commit ──▶ message "Verified report ..." to PM
                │
                │ 4. cstan request-review <report-id> [role]
                ▼
        controller spawns reviewer at that commit ── cstan review pass|findings "<text>"
                │                                          (reviewer ended by controller)
                ▼
        message "Review ..." to PM
          ├─ findings ─▶ PM sends them to the developer ─▶ new verified report ─▶ new review
          └─ pass
                │ 5. cstan integrate <report-id>...
                ▼
        controller merges the reports onto  capstan/integration/<id>  (no checkout)
          ├─ conflicted ─▶ nothing left behind; PM assigns a developer to resolve, then report/review again
          └─ merged
                │ 6. cstan request-review <integration-id> [role]
                ▼
        review of the integration  ── pass
                │ 7. YOU merge the integration branch into the project's HEAD
                │ 8. cstan integrate confirm <integration-id>   (or: integrate discard <id>)
                ▼
        controller removes the integration branch;  PM: cstan release <agent-id>
```

Step by step, as the code behaves (`src/commands.ts`, `src/controller/core.ts`, `src/launcher.ts`, `src/reviews.ts`, `src/integration.ts`):

1. **Spawn.** `cstan spawn <role>` creates an agent of that role in a new Herdr pane and git worktree on a branch named `capstan/<agent-id>-g<generation>`, cut from the project's HEAD. It is refused with `worker_limit` when `limits.max_workers` workers are active.
2. **Send the task.** `cstan send <agent-id> "<text>"` queues a message. See [Messaging](#messaging). The task should state the acceptance criteria and that the worker commits on its own branch and never pushes or merges.
3. **Report.** When finished, the worker runs `cstan report <40-char-commit> "<one-line summary>"`. The controller checks that the commit exists on the worker's branch, is not older than the branch's start, and is on that branch; otherwise the report is recorded as `rejected` and the command fails. An accepted report is announced to the PM as a message from `controller` starting with `Verified report`. This is a fact the controller checked, not a review. A repeat report of the same commit writes nothing new. Reports are rate-limited to 10 a minute per agent.
4. **Request review.** Only the PM runs `cstan request-review <report-id> [role]`. The report must be accepted. The controller spawns a fresh Verifier (the named role, or the configured one) at that commit and sends it a `Review request`. The reviewer answers exactly once with `cstan review pass "<text>"` or `cstan review findings "<text>"`, after which the controller ends it. The PM gets the verdict as a `Review` message from `controller`. On findings the PM sends them to the developer, which amends its commit, reports the new full commit id and gets a new review round. An amend replaces the commit id: a report naming an amended-away id is rejected as not on the branch, the earlier reviewed commit stays in the ledger as an earlier report, and the new report needs a new review. After a passed review the developer amends only when findings arrive, and always reports again.
5. **Integrate.** `cstan integrate <report-id>...` (PM or operator) merges reports that each have a passing latest review, in the order given, onto a new branch `capstan/integration/<integration-id>` cut from the project's HEAD, without checking anything out. The branch is the project's HEAD plus one squash commit (subject `<type>: <title>`, body listing the reports and agents); the per-report commits stay only on the worker branches. The answer is `merged` (branch and head commit), `conflicted` (the report and files) or `failed`. The controller never resolves a conflict and leaves nothing behind: the PM assigns a developer to resolve it as a new candidate, then report, review and integrate again.
6. **Review the integration.** A merged result needs its own review: `cstan request-review <integration-id> [role]`.
7. **You merge.** After a passing review, merge the integration branch into the project's HEAD yourself. Capstan never pushes or merges your branch.
8. **Confirm or discard.** `cstan integrate confirm <integration-id>` is accepted only when the integration is `merged`, no review of it is open, its latest review passed, and its branch is already in the project's HEAD (otherwise `not_in_head`); after you fast-forward the branch, this checks that the squash commit is in HEAD; the controller then removes the branch. `cstan integrate discard <integration-id>` drops a merged integration you will not use.
9. **Release.** `cstan release <agent-id>` ends a worker and frees its pane and worktree. The branch is kept when it holds commits.

**Reports a confirmed integration covers.** When an integration is confirmed (and again at every controller start), the controller also marks reports it did not merge but whose work it holds, so a package is not left open. A report is covered `how`: `ancestor` (its commit is in the head's history), `tree` (every path it changed is untouched since), `merge` (`git merge-tree --write-tree` of the head and the report adds nothing, or conflicts only because a later commit edited the same lines and kept every word of the report's added lines; needs git 2.38, else the rule is off and logged), or `integration` (the report belonged to an earlier integration of any final state with a stored head, and that head is an ancestor of a report commit this integration merged). Limitation: `integration` means "built upon", not "still present": a later revert inside the confirmed integration is not detected, and `merge` on conflicts is a line heuristic.

States recorded in the ledger: reports `accepted` or `rejected`; reviews `started`, `passed`, `findings`, `failed`, `cancelled`; integrations `running`, `merged`, `conflicted`, `failed`, `confirmed`, `discarded`.

## Messaging

All agent-to-agent text goes through the controller, one strict FIFO per recipient.

- **Send.** `cstan send <agent-id|@pm> "<text>"`. Plain text only (a message may not start with `/`, `!`, `#`, `?` or `@`), size-limited, and may not contain a line that imitates a Capstan message frame. A worker (Developer or Verifier) may send only to the PM. Sending to yourself is refused.
- **Delivery to a worker.** The controller types the message into the worker's pane as `[capstan message <message-id> from <sender>]` followed by the text. It waits while the worker is busy, blocked at a prompt, or has text in its input line (deferral reasons `agent_busy`, `agent_blocked`, `input_not_empty`). After `timers.max_deferral_seconds` it reads and logs the typed text, clears the input line and sends. A message that waits longer than `timers.max_busy_deferral_seconds` expires and the PM is told.
- **Delivery to the PM.** Nothing is typed into the PM's pane. The PM reads with `cstan inbox` (the next message plus any read but unacknowledged) or `cstan wait` (blocks up to the host's wait timeout, default 90 s, and prints unacknowledged messages). When the PM is idle and unread mail has waited `timers.pm_wake_after_seconds`, the controller types `Run cstan inbox`.
- **Worker pull.** A worker can also read its mail itself. `cstan inbox` moves every queued or deferred message to `sent` (oldest first; it stops at an expired or failed one, which is the PM's to resolve, and pulls nothing while the worker is paused) and prints them with the unacknowledged ones. `cstan wait` blocks up to the host wait timeout until mail arrives, then prints it like `inbox`; a worker uses it when it has nothing else to do, never a shell polling loop. (`pullPending` in `src/controller/core.ts`, `pullFor` in `src/commands.ts`.)
- **Unread notice.** Any other successful worker command (not the PM, not `inbox`/`wait`) also prints `notice: N message(s) wait for you (oldest M min): run cstan inbox` on stderr when messages are queued, deferred, sent or unacknowledged (`withNotice`). `--json` output carries the same as an `unread` field.
- **Report refusal.** `cstan report` is refused with `unread_messages: ...` (naming the message ids) while a message to the reporter that was queued at or before the reported commit's committer time (each message's queue time is compared with it) is still queued, deferred, sent or unacknowledged. The worker runs `cstan inbox`, acts on and acks them, then reports again.
- **Hook per host.** A Claude worker gets a `PostToolUse` hook (`cstan inbox --hook`, matcher `*`, 5 s timeout) that adds a short "N Capstan message(s) are waiting" line to the model's context after a tool call when mail waits. It is read-only, never prints a message body, and fails silently; it prints nothing and exits 0 when `CAPSTAN_TOKEN`, `CAPSTAN_SOCKET` or `CAPSTAN_AGENT_ID` is missing or the daemon is unreachable. It is not shipped when the role has `hooks = "off"`. Codex and OMP workers get no hook: they have the pull commands, the unread notice and the report refusal only.
- **Mail rules in the prompts.** Every worker prompt (Developer, Verifier, Architect, Supervisor) tells the worker to run `cstan inbox` at the start of each step, before every report and between long steps; to acknowledge each message as soon as it is read (any order is accepted); never to poll with a shell loop but to use `cstan wait`; after an `unread_messages` refusal to read, act, ack, then report again; and to run `cstan inbox` now when a notice or the hook says messages wait. The PM prompt says a message to a busy worker waits in its queue until the worker runs `cstan inbox` or becomes idle and the controller types it.
- **Acknowledge.** `cstan ack <message-id>`. Reading does not acknowledge. Typed delivery sends the next message to a worker only after the current one is acknowledged; messages a worker pulled may be acknowledged in any order.
- **Message states:** `queued`, `deferred`, `sent`, `acked`, `acked_late`, `unacked`, `expired`, `cancelled`, `failed`.
- **Delivery problems.** When a message to a worker becomes `unacked`, `expired` or `failed`, that worker's queue is blocked behind it and the PM gets a `Delivery problem` notice naming the command to run:
  `cstan resolve <message-id> retry|skip|cancel ["<note>"]` — `retry` types it once more (the recipient may already have received it), `skip` counts it handled, `cancel` drops it. `cstan cancel <message-id>` is the same as `resolve ... cancel`.
- Operators can read an agent's mailbox with `cstan inbox <agent-id>`.

## Worker lifecycle

- **Spawn:** see step 1 above. `cstan spawn` and `cstan release` can take minutes; run them with a long timeout, and after any timeout run `cstan status` before retrying (the worker may already exist and counts against the limit).
- **Release:** `cstan release <agent-id>` removes the pane and the worktree. The answer reports `paneClosed` and `worktreeRemoved`; `false` means that cleanup is still pending and the next spawn or release finishes it (`cstan status` shows `cleanupFailed` and `orphanPanes`). `agent_not_active` means the worker is already gone. Reviewers are released by the controller after they answer.
- **Replace:** `cstan replace <agent-id>` replaces a lost or stuck worker. The controller releases it (when still running) and starts a new agent of the same role with a new id, a new branch that starts at the predecessor's last accepted report (else HEAD), and a seed built from the ledger (its instructions and their states, accepted reports, open findings). Nothing is re-sent; the PM sends what still matters. It is refused while an integration is running and for an agent that was already replaced.
- **Lost:** when Herdr no longer finds an agent's pane or process (or its pane was already gone when the daemon started), the controller ends the agent in the ledger and sends the PM an `Agent ... is lost` message with its branch and its unacknowledged messages. The controller never replaces it by itself: use `cstan replace` or `cstan release`.
- **Stalled / blocked:** the controller watches worker activity. `Agent stalled` means no activity while working for `timers.stall_after_seconds`. The alert is suppressed while a tool process under the agent (a shell started by it, such as a test run or build, with its descendants; MCP servers are not counted) used CPU within that window, so a long silent command is not reported. On Linux the CPU counted is `utime + stime + cutime + cstime` from `/proc/<pid>/stat`, so short-lived children that already exited, such as test-file runners, count; elsewhere it is the CPU time `ps` reports. A hung process that uses no CPU, an agent with no tool process, or a failed process probe still alerts after `timers.stall_after_seconds` (`suppressActiveStalls` in `src/controller/messaging.ts`; the CPU probe is in `src/herdr/process-activity.ts`); `Agent blocked` means it waits at a dialog or permission prompt only a person can answer. Use `cstan observe <agent-id>`, then replace the worker or tell the user. Nobody answers another agent's permission prompt.
- **PM restart:** `cstan pm restart` (operator) replaces the PM session with a new one that starts from a summary of the ledger (open work, unacknowledged messages).

## Pause and resume

`cstan pause [<agent-id>] --reason "<text>" [--interrupt]` and `cstan resume [<agent-id>] --reason "<text>"` hold and release work. Only the operator and the active PM may use them; a worker gets `forbidden`. The reason is required and is shown in `cstan status` and `cstan dash`. A pause is kept in the ledger (table `pauses`) and survives a controller restart; each pause and resume is also a ledger event with its reason, actor and time, and an operator's run pause or resume adds an operator action and a controller notice to the PM.

- **Agent scope** (`cstan pause <agent-id>`): the agent must be active, and a PM may not pause itself. Messages to it stay queued (or deferred) in sequence order: none is typed into its pane, none expires, and no new deferral is recorded. A paused PM is handed nothing new by `cstan inbox` or `cstan wait`. After `cstan resume <agent-id>` the queue is delivered by the usual rule, one message per ack, and a deferred head's deferral clock restarts.
- **Run scope** (`cstan pause`): every worker (developers, the architect, reviewers and the Supervisor) is held the same way; the PM keeps receiving, because it coordinates and must see notices. While the run is paused these are refused with an error that says the run is paused and gives the reason: `cstan spawn`, `cstan launch` (not `cstan pm restart`), `cstan plan assign`, `cstan request-review`, `cstan integrate` and the execution of an operator proposal that an auto rule, a session-grant match or full auto approved (it is cancelled, the refusal is recorded on the proposal with the reason and the PM is told; proposing is still allowed). A proposal the PM approved one-off with `cstan op decide` still runs, restart included, and so does one whose decide opened a session grant. `cstan resume`, `status`, `dash`, `observe`/`peek`, `inbox`/`ack`/`send`, the PM's `wait`, `release`, `replace`, `pm-restart` and `op decide`/`cancel`/`show` keep working. `plan assign` to a paused agent and a Supervisor finding about a paused agent are refused as well. Work-item commands that need an active run (`work.create`, command dispatch, input revisions) stop too.
- **Timers.** For a paused agent the controller produces no stalled or blocked notice, PM wake or operator notification, a deferred head does not expire, and finding deadlines and escalation do not advance; they start again from the resume. Loss detection still runs, and the loss notice and status mark the agent `(paused)`. A message already sent but not acknowledged still goes `unacked` on the worker ack timeout, because the agent is alive and can ack.
- **`--interrupt`** (pause only) sends exactly one `Esc` to each worker Herdr shows working (with an agent id, to that agent), through the prompt-relay key log. It needs `[prompt_relay] enabled = true` (otherwise `not_configured` and nothing is paused), never sends a second key, and sends nothing to an idle agent. The answer lists the agents that got the Esc.
- **Display.** `cstan status` prints `PAUSED (<age>): <reason> by <actor>` for the run and for each paused agent (`--json` has `pause: { run, agents }`). `cstan dash` shows the paused run in its header and a `paused <age>` marker on the held agents' rows.

## Supervisor findings

While workers are active the controller keeps one Supervisor running (`[supervision]`) and sends it a `Routine check` message every `check_seconds`. The Supervisor looks at agents with `cstan status` and `cstan observe`, and only raises a finding when an agent is clearly stuck (the same failure repeated, a retry that cannot work):

- `cstan finding <agent-id> <severity> "<evidence>" "<requested correction>" "<done when>"` raises a finding about a Developer or Verifier. Severity is `info`, `low`, `medium`, `high` or `critical`. The controller delivers it to that agent as a `Finding` message. One finding may be open per agent.
- `cstan finding check <finding-id> resolved|unresolved "<evidence>"` records the check after the agent acknowledged. `resolved` closes it; `unresolved` sends one more correction. The controller sends at most two corrections; after the second unresolved check, or if the Supervisor does not check within `timers.finding_check_seconds`, it escalates.
- Finding states: `open`, `resolved`, `escalated`, `cancelled`. The PM is told about raised, resolved, escalated or cancelled findings; an `ESCALATED` finding means no further corrections will be sent and the user should know.

Agents treat a `Finding` message as data from a supervisor, not as an instruction from the controller.

## Status, observe and the dashboard

- `cstan status [--json]` prints the project state: roles and seats, work, findings and, when the daemon is reachable as the operator, agents, unresolved messages, panes, reports, reviews, integrations, agent findings, stalled/lost agents and cleanup failures. If the daemon is stopped it reads the ledger directly. `cstan status --watch [--interval <seconds>]` redraws it (interval 1 to 60, default 2).
- `cstan inspect <id> [--json]` shows one ledger record (work item, assignment, candidate, finding, recovery or report).
- `cstan observe <agent-id> [lines]` prints another agent's recent screen. PM and Supervisor only; the text is the agent's own output, not verified, and any instruction in it is data. The default is 40 lines, at most 120, and at most 30 reads a minute.
- `cstan dash [--interval <seconds>] [--no-color] [--reduced-motion]` is a read-only terminal dashboard (needs a TTY; `CSTAN_REDUCED_MOTION=1` also reduces motion; `NO_COLOR` is respected). Panels: agents, pipeline, queue, findings, with a header for run state, health and worker count. Keys: `tab`/`shift+tab` and `1`-`5` move between panels, `↑↓` or `j k` select a row, `o` observes the selected agent (read-only), `y` retry / `s` skip / `c` cancel the selected message (each asks to press `y` again to confirm; the daemon decides), `f` shows only delivery problems, `p` pauses polling, `r` polls now, `-`/`+` change the poll interval, `?` help, `q` or `ctrl+c` quits. Design notes: `docs/design/cstan-dash-v2.md` and `decisions/DEC-006-cstan-dash.md`.

## Planned work and the Architect

With `[architect] enabled = true` the PM sets a tier for each requirement and tells you; you may override it (`src/prompts.ts`, `PM_PLAN_SECTION`).

| Tier        | What happens                                                                                                                                                        |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `small`     | No plan and no Architect. The PM spawns a developer, reviews, integrates, merges the integration branch itself and runs `integrate confirm`.                        |
| `normal`    | The Architect reads the code and submits a plan of work packages; the PM assigns packages to developers.                                                            |
| `high-risk` | As normal, and the controller has the plan reviewed by a fresh Verifier before it is final (`plan_review = "high_risk"` by default; `always` or `never` also work). |

The flow for a normal or high-risk requirement:

1. The PM runs `cstan plan open normal|high-risk "<title>"`, spawns the Architect and sends it the plan id and the requirements.
2. The Architect submits one plan with `cstan plan submit <plan-id> "<json>"`: packages with owned files or areas, interfaces, dependencies, an estimate, acceptance criteria and risks. The controller refuses overlapping owned areas that are not ordered by a dependency, cycles and more than `max_packages` packages (`src/plans.ts`).
3. When the plan is approved the PM gets `Plan <plan-id> approved`, spawns a developer per package and runs `cstan plan assign`; the controller sends the package text itself.
   - **Dependency gate.** A package can be assigned only when each package in its `depends_on` is _reviewed_ (its report has a passing review) or _integrated_. Otherwise `plan assign` is refused with `dependencies_unmet: package <id> depends on <d1> (assigned), <d2> (unassigned)...`; a cancelled dependency counts as unmet. `cstan plan assign <plan-id> <package-id> <agent-id> --early "<reason>"` assigns anyway: the reason and the unmet list go into the ledger event and the developer's message says to build against the interfaces the plan states.
   - **Integration order.** `cstan integrate` refuses a plan package's report (`integration_order`) unless each dependency's report is already in a merged or confirmed integration, or comes earlier in the same list. Reports outside plans are not affected. A dependency that is not cancelled but has no report yet (unassigned, or assigned and not reported) is refused with `needs <dep>, which has no report yet`. A cancelled dependency, a cancelled package, and a cancelled or superseded plan are skipped here: assign asks whether the interface you build against will exist, so a cancelled dependency stays unmet there; integration asks whether anything must merge first, and for a cancelled dependency nothing will.
4. Developers may ask the Architect questions with `cstan send`; the Architect answers directly but assigns no work.
5. Reports and reviews of assigned packages go to the Architect, which requests reviews, runs `cstan integrate`, has the integration reviewed and runs `cstan plan signoff`. On a conflict it asks the PM for a developer.
6. The PM gets `Plan <plan-id> signed off` with the integration branch and tells you. **You** merge it; the PM then runs `cstan integrate confirm`. The Architect never merges and never confirms.

`cstan plan show [<plan-id>]` lists plans, packages, assignees and derived progress. The operator can cancel a plan or package with `cstan plan cancel`. The design record is [`docs/design/architect-role.md`](../design/architect-role.md).

## Prompt relay

Workers sometimes stop at a permission prompt that only a person can answer. With `[prompt_relay] enabled = true` the PM can show you that prompt and type your answer:

1. On an `Agent blocked` notice the PM runs `cstan prompt show <agent-id>`. It prints the prompt inside an untrusted-data frame, the numbered options (each marked `acceptsText` and `widensPermissions`), a hash and an expiry (`capture_ttl_seconds`, default 600).
2. The PM shows you the exact prompt and options in a picker; an option that widens permissions is labelled as such.
3. Only with your choice, the PM runs `cstan prompt answer <relay-id> --hash <hash12> option <n>`, `esc` or `text <text>`. A prompt that changed since it was shown is refused (`prompt_changed`), as are unknown options and unsafe text.

An unrecognised blocking dialog is relayed too, but as Esc only. Some Claude screens (for example `Teach auto mode about your environment?`) have no input box, are not a permission prompt and may not turn Herdr `blocked`. `cstan prompt show` relays one only when it is a Claude screen whose last non-empty line contains `Esc to cancel`, with no input box, no parsed permission prompt, no numbered option rows, no stray control characters and a Herdr state other than `working`. The output says `kind: dialog` and offers exactly one option, Esc; option numbers, text, Enter and arrows are refused before any key. `prompt answer ... esc` sends one Esc, never a second, then reports `inputReadable`: when false the input box is not readable yet (not a failure), so look with `cstan observe`; the PM's waiting message is delivered once the input box reads again. The early `Agent blocked` notice says whether `prompt show` can relay the screen.

While the table is absent or `enabled = false`, both `cstan prompt` subcommands are unavailable and the PM prompt does not mention them. Only the active PM may relay.

## Nexora tracking

With a `[nexora]` table whose `track` is `ask` or `always`, the PM mirrors work into Nexora as an epic per requirement and a story per work package. Capstan itself never calls Nexora: the PM writes to Nexora with its own tools, and the ledger keeps only the ids and the states the PM recorded with `cstan link`. `cstan status` (and `cstan plan show`) print a `Nexora drift` section comparing what was last written with what the ledger now wants, so the PM writes only what differs. `track = "never"` removes every Nexora instruction from the PM prompt; connection details stay in `.nexora.toml`, which Capstan never reads.
