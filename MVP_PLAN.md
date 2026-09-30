# Capstan MVP Plan

> **Notice (2026-09-30, PM-20): superseded in part.** The direction changed: no Docker, no per-role network or firewall, no OMP-extension bridge; interactive agents run in Herdr panes and git worktrees. `MVP_PLAN_V2.md` and `decisions/DEC-005-capstan-v2-direction.md` are now the plan of record. The Docker, network, egress and OMP-bridge material below (mainly sections 5.2 and 9 and the M1 qualification text) is history. The controller core, ledger and recovery material is reused. Nothing below was edited.

## 1. Purpose and status

**Product:** Capstan. **CLI executable:** `cstan`. **Project directory:** `../capstan/`, separate from claw8.

**Status:** PM-8 supplies the four-seat M2 path; PM-9 adds M3 restart reconciliation and operator controls. PM-10 owns the real-runtime M3 failure-injection suite and exit gate; implementation or unit tests alone do not establish that gate.

### PM-5 implementation boundary

PM-5 adds a TypeScript/SQLite controller core, transactional migrations, explicit transition/capability tables, versioned and idempotent mutation handling, assignment-bound input snapshots, an outbox/receipt ledger, candidate-bound evidence, runtime/finding/recovery records, and an adapter for the existing M1 Unix-socket bridge. The adapter waits for the controller to commit the exact M1 `accepted` receipt before sending the separate `start` request. M1 `submitted` means the prompt was sent; `working` means the assigned agent started; `completed` is the worker report that moves Developer/Verifier work to `awaiting_verification`.

M1 `submitted`, `working`, and `completed` receipts are accepted for PM, Developer, Verifier, and Supervisor assignments when the transition table authorizes the role. The controller accepts a PM or Supervisor report only after the matching completion receipt has non-empty text, is durable, and the assignment authority is explicitly contained. Accepted non-candidate work can satisfy a dependency that does not pin a candidate ID. Dependency edges may be revised with an audited mutation while pending, blocked, or ready work has no assignment; after assignment they may be replanned only while blocked and after all prior assignment authority is contained. Existing assignment input snapshots remain immutable. A run cannot enter `completed` while work remains open or assignment authority is active or unknown, and non-active runs reject new work.

While the controller remains alive, an `attempting` command may be retried by writing a new immutable outbox attempt and resending the same command ID and payload; the M1 bridge deduplicates that identity. After restart, ambiguous delivery or worker state becomes `unknown` and is not blindly resent. Project inputs cannot be revised while work is running/awaiting verification or any assignment authority is active/uncertain. Pending, blocked, or ready work with no active/uncertain authority may be rebound to current inputs; a stale accepted prerequisite remains stale until an authorized planner removes or replaces its dependency after all prior authority is contained, and historical receipts and assignment input snapshots are never rewritten.

The controller takes a nonblocking kernel `flock` on a project lock inode. This prevents a second cooperating controller while the lock path remains intact; same-user unlink/replacement of that path is explicitly outside the accepted trust boundary (DEC-004). Do not describe this as protection from hostile same-user processes or an independent Herdr caller.

On restart, commands without a durable completed report are not resent; their assignments are revoked with authority `unknown` and work is blocked. A durably completed report keeps its `reported` assignment state but has `unknown` authority; after `confirmContainment` attests proof, the blocked work returns to `awaiting_verification` for acceptance. A known pre-start assignment cannot be marked contained; after restart, dispatch-attempting or acknowledged commands without durable start intent require bridge-state reconciliation before containment. Ambiguous pre-start receipts cannot bypass that guard. Containing running work blocks it so an authorized `markReady` and bounded replacement can proceed; late receipts from contained authority are durably acknowledged and audited without advancing old work, preserving the role sequence for replacements. Dispatch and start refuse contained or uncertain authority. `confirmContainment` records an operator-requested controller attestation and its `proofRef`; it does not independently verify PID, process-tree, container, or cgroup containment. Recovery remains blocked until an operator supplies that attestation and stays within the configured recovery cap.

For an uncertain pre-start command, `M1BridgeAdapter.reconcilePrestartAndContain` queries the same command ID through `get` and records its fresh snapshot with an operator's separate quiescence proof before restoring readiness. `get` alone never re-authorizes a worker; an M1 `completed` snapshot without a durable controller report permits only containment and bounded replacement, not report acceptance. An M1 `running` snapshot is not accepted as pre-start reconciliation. Actor credentials are controller-generated and returned on actor creation; the project owner credential remains an operator-supplied bootstrap secret.

Build a small but real autonomous delivery loop using separate OMP sessions, Herdr hosting, a durable controller, and a dedicated Workflow Supervisor. Demonstrate that the team can complete a bounded software task, preserve ownership through failures, and recover from a repetitive failure without losing its next action.

This MVP realizes the feasibility and durable-coordination stages of the full product plan. It does not replace the full role hierarchy or establish that arbitrary tasks can be delivered autonomously. The full product still includes Product Owner, Project Manager, Architect, Developer, QA, Reviewer, and Integration responsibilities under Supervisor oversight.

**Naming:** Capstan and `cstan` are selected. `cap` conflicts with Capistrano and Cap; `capstan` is already an OSv executable. The product-name overlap is known. An initial search did not establish an existing `cstan` CLI, but package, domain, command, and trademark availability are not guaranteed.

## 2. Questions the MVP must answer

| ID  | Hypothesis                                                                 | Required proof                                                                                                                                                         |
| --- | -------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H1  | Separate agents can collaborate without relying on one shared conversation | Real PM, Developer, Verifier, and Supervisor sessions exchange assignment-bound outputs through the controller.                                                        |
| H2  | Durable state prevents forgotten ownership and next actions                | After session replacement or controller restart, status and reconstructed assignment context identify the same accepted inputs, owner, blocker, and next legal action. |
| H3  | Independent supervision can break an unproductive loop                     | Supervisor identifies an injected repetitive failure, routes a correction, observes acknowledgement, and verifies recovery or produces a bounded escalation.           |
| H4  | Failure recovery does not introduce duplicate execution                    | Ambiguous delivery and worker replacement never create two authoritative writers; stale reports cannot advance state.                                                  |
| H5  | Independent verification prevents premature completion                     | A deliberately incorrect candidate is rejected, corrected, and independently verified before parent acceptance.                                                        |
| H6  | Coordination cost is measurable and potentially worthwhile                 | Report complete team usage, elapsed time, and human intervention against matched claw8 baseline tasks; distinguish correctness gains from token savings.               |

A successful happy path alone answers none of the recovery questions. A successful recovery alone does not prove efficiency.

## 3. Bounded MVP scope

### Included

- One operator, one local controller, one project, and one active parent task at a time.
- Four stable seats: Project Manager, Developer, independent Verifier, and dedicated Workflow Supervisor.
- A distinct OMP session/profile per active seat, visible through the validated Herdr integration.
- A versioned PM plan with at least two sequential child slices, an explicit dependency, and parent acceptance criteria.
- One active implementation writer at a time. Verification and supervision remain separate roles, not Developer self-review.
- Typed assignment delivery and acknowledgement, durable handoffs, evidence references, and explicit state transitions.
- SQLite-backed operational state, append-only events, and a transactional outbox.
- Independent checks against exact candidate revisions, followed by parent-level verification of the final local candidate.
- Event-driven Supervisor findings, correction tracking, resolution verification, and bounded escalation.
- Controller restart, worker replacement, stale-result rejection, and explicit degraded-supervision handling.
- Operator status, pause, resume, cancel, and inspect commands.
- Machine-readable run evidence and usage observations, including unknown-usage markers.
- A real demonstration on a disposable repository with injected failures in the real selected runtime path.

### Deferred, not removed from the product

- Separate Product Owner and Architect agents. The operator supplies an approved brief and a deliberately small, prebounded task. Architectural ambiguity blocks the run instead of being silently absorbed by the Developer.
- Separate Reviewer and Integration agents. The Verifier covers independent behavioral acceptance only; the MVP makes no full peer-review or security-review claim. The controller performs deterministic candidate composition and dispatches final verification.
- Multiple concurrent developers, conflicting-slice scheduling, and general multi-project operation.
- Tracker adapters, automated PR creation, push/merge, deployment, and production credentials.
- Multi-host scheduling, browser dashboard, rich TUI, long-term product knowledge retrieval, and automatic policy improvement.
- General hostile-code or multi-tenant security qualification.

**Delivery target:** an independently accepted local commit or local branch tip plus an evidence report. A remote PR, merge, or deployment is not needed to prove this MVP.

**Autonomy boundary:** after approving the task brief and starting a run, the operator should not need to tell agents which normal workflow step comes next. Scope changes, unresolved disputes, exhausted recovery limits, and unavailable containment mechanisms require a visible blocked state.

## 4. Team and authority contracts

| Actor               | Responsibility                                                                        | Required outputs                                                               | Prohibited authority                                                                   |
| ------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| Operator            | Provide the product brief, acceptance criteria, authorized repository, and limits     | Approved input revision; explicit decisions on escalations                     | No invisible mid-run edits to accepted scope                                           |
| PM                  | Interpret the task, propose slices and dependencies, respond to coordination findings | Structured plan; clarification requests; bounded replan proposals              | Cannot bypass dependencies, accept its own unverified delivery, or mutate the database |
| Developer           | Implement the currently authorized slice                                              | Candidate identity; implementation evidence; blocker or completion report      | Cannot approve acceptance, widen scope, or continue under revoked authority            |
| Verifier            | Exercise slice and final-parent acceptance independently                              | Candidate-bound pass/fail observations and actionable rejection reasons        | Cannot modify the candidate while verifying or accept a different revision             |
| Workflow Supervisor | Observe every active role and handoff; diagnose workflow issues                       | Findings, routed corrections, verified resolutions, escalation recommendations | Cannot implement fixes, rewrite product priorities, or directly change assignments     |
| Controller          | Enforce ownership, scheduling, policy, transitions, persistence, and recovery         | Durable state, dispatch receipts, audit events, operator status                | Cannot invent model judgments or infer acceptance from terminal idleness               |

All agents submit proposals or reports through authenticated, role-scoped controller interfaces. Only the controller commits authoritative transitions. Role names inside message bodies do not establish identity.

The Supervisor is present from the first multi-agent demonstration. OMP advisors, ordinary logging, PM self-monitoring, and deterministic timeouts are not substitutes for this seat.

## 5. Foundation and runtime decision gate

### 5.1 Inspect before choosing a foundation

Inspect a pinned OpenRig revision for these decisive capabilities:

1. Supported OMP integration and compatibility with Herdr-hosted sessions.
2. Assignment identity, delivery deduplication, and durable acknowledgement.
3. Atomic handoff and restart reconciliation.
4. Ability to implement a separate Supervisor with routed, tracked findings.
5. Ability to enforce a single writer and reject superseded reports.

Record an extend/reuse/independent-controller decision before building the coordination core. Reuse suitable mechanisms rather than inventing parallel conventions. Do not build two complete competing prototypes. If adopting source, preserve its license and attribution requirements.

If an independent core is selected, prefer a small TypeScript service, a local CLI, SQLite/WAL, and file-backed artifacts. Select a maintained SQLite binding compatible with the pinned Node runtime. Avoid a message broker, plugin framework, distributed scheduler, or generic workflow language in this MVP.

### 5.2 Preferred runtime path

Validate an interactive OMP session in a dedicated Herdr pane, controlled through a narrow OMP extension bridge:

```text
Operator -> cstan -> Controller + SQLite/outbox
                         |
              authenticated runtime bridge
                         |
            Herdr-hosted OMP role sessions
             PM / Developer / Verifier / Supervisor
                         |
             typed reports and observations
                         v
                      Controller
```

The bridge carries assignment identities and structured events; terminal text is a diagnostic artifact, not the authoritative message protocol. The controller is the expected sole command authority during normal Capstan assignments, but Herdr does not enforce that exclusivity: an independent local client can start or prompt an OMP agent and bypass controller dispatch and recovery records. The user explicitly accepts this as a residual operational risk for M2–M4; do not claim technical exclusivity. Herdr supplies hosting and lifecycle observations, not semantic task completion.

**Feasibility must demonstrate:** launch, exact profile/cwd/argv handling, ready observation, correlated prompt delivery, acknowledgement, completion report, interruption, process-tree termination or equivalent containment, reconnect, and safe replacement. Also verify the tested recovery order: gracefully exit OMP, observe it absent, close its pane, then restart Herdr with agent restore enabled. This prevents that closed pane from restoring in the tested case; it does not fence new callers.

If a preferred-path property remains unmet, stop and record the exact gap. Continue only after explicit user acceptance is recorded in the decision and acceptance criteria; the accepted caller risk does not authorize controller-owned OMP RPC, terminal scraping, or a second command authority.

**Selected-source qualification: PASS, 10/10/10**, runtime commit `69e0ac7`; evidence at `~/.local/state/capstan/m1-herdr-qualification/capstan-m1-herdr-PFTKA2/qualification.jsonl`. Herdr 0.9.0, OMP 18.3.1, Node 24.6.0, Docker 29.5.0, pinned Ubuntu image. Ten selected acknowledgements 9,423.685–9,768.890 ms; maximum progress silence 20,077.115 ms; containment 3,073.842–3,229.873 ms; selected quiescence 9,689.620 ms. All ten samples in each gate passed; cleanup left zero containers, policies, networks, or errors. The journal records staged helper and TLS SHA-256 values before execution.
Qualification procedure: `scripts/m1-herdr-qualification.mjs` exercises ten launch-to-ack probes, ten long-running Bash progress probes, and ten containment/restoration probes. It verifies same-workspace writer stability before replacement, controller-owned fsynced receipts, exact Herdr-hosted OMP peer credentials, pidfd-checked proxy identity and exit, staged helper/TLS source digests and scoped provider egress through FORWARD/DOCKER-USER, durable tool start/end evidence and completion, immediate handling of unknown outcomes, exact progress bytes, receipt replay after reconnect, cgroup emptiness, and mount removal. Cleanup retains containers until policy removal and reports bounded cleanup failure. Entire-controller process death/rehydration remains M3.
Receipt ingress buffers raw bytes through the line delimiter, applies the 1 MiB limit to the complete frame, rejects malformed UTF-8, an initial BOM, and extra frames; TLS egress validates the full extension block, rejects duplicate SNI extensions, and accepts only ASCII hostname bytes with case-insensitive hostname comparison. Firewall verification tokenizes quoted iptables rules, requires the exact unconditional FORWARD-to-DOCKER-USER route without preceding bypass rules, and rejects bypasses before scoped policy. Policy removal requires a stopped worker; paused workers are stopped before rule removal. Non-public IPv4 ranges are rejected. Egress prepare and cleanup acquire the same atomic per-container reservation; cleanup cannot report absence while preparation owns the container, and competing lifecycle operations fail closed. Proxy termination caller timeout exceeds its bounded TERM/KILL wait. The bridge authenticates only peers outside the worker PID namespace (controller peers appear as PID 0 inside the worker); worker-originated get/abort connections are denied, while independent host same-user callers remain the separately accepted DEC-003 risk. Terminal bridge responses flush then destroy the client socket; queue admission is bounded, pipelining is rejected, and accepted-but-unacknowledged commands remain unknown and locked. Progress PASS requires each tool-start/end reference to match its fsynced journal entry. Empty or whitespace-only assistant output leaves the assignment unknown and locked; controller recovery discards only incomplete abort-intent tails. Provider…
Dispatch uses an explicit two-step confirmation: the controller receives and validates the durable `ack`, then sends `start`; only `start` can trigger `sendUserMessage`. Repeated `start` returns the existing authorized state without resubmitting, while an ambiguous/replayed outcome remains unknown and locked.
Controller dispatch and `start` requests opt into a single-response mode: the bridge closes the request socket after one response, and the adapter waits for EOF while rejecting duplicate, malformed, or incomplete trailing frames. Legacy callers that omit the flag retain progress streaming; TCP chunk grouping is not treated as a message boundary.
The controller writes each DB-accepted receipt to the private role journal and fsyncs it before acknowledging the worker; adapter startup rebuilds that journal from durable receipt rows so replacement workers continue the role sequence.
If OMP dispatch initiation throws after the accepted receipt, the bridge persists `dispatch_error` and marks the row `unknown`, but keeps the active slot locked: a synchronous throw does not prove no work started. Duplicate delivery never resends, a queued authorized abort prevents the deferred send, and subscribers receive an unknown frame rather than waiting indefinitely. `scripts/m1-herdr-bridge.test.mjs` exercises durable receipt, query, retry, abort race, subscriber notice, and no-second-send invariants.
If controller receipt persistence also fails, the bridge still marks the identity `unknown` in memory and retains the active slot, blocking new assignments until recovery rather than reporting false acknowledgement.
The bridge persists and fsyncs the `working` receipt before publishing `durable: true, state: working`; if persistence fails, it reports unknown and keeps the slot reserved. The regression asserts ordered accepted/submitted/working journal records and the failure transition.
If `sendUserMessage` succeeds but the `submitted` receipt is unavailable, a tool start/end receipt cannot be persisted, or a completion receipt cannot be persisted, the bridge marks the row `unknown` and keeps the active slot; it never records a false terminal dispatch error or reports indefinite `working`. Duplicate delivery does not resend. Subscribers receive unknown immediately on uncertain outcomes. The abort intent writer checks full writes and rolls back a partial tail before authorizing controller abort.

Completion is correlated only to the latest assistant `turn_end` observed after `agent_start`; aggregate `agent_end.messages` can contain stale prior-run output. Missing, empty, or whitespace-only current-turn text is recorded as nonterminal `agent_end_without_reply` when durable storage is available; otherwise the bridge still reports unknown to subscribers and retains the active slot through controller replay and bridge restart. A controller-authorized durable `aborted` receipt records revocation intent, but does not prove worker/tool containment or release the active slot. Replacement requires container/cgroup teardown and stale endpoint removal. An unaccepted abort receipt cannot revoke work.
The prior post-parser qualification PASS ran before the subsequent dispatch-error state correction and is not final evidence: Herdr 0.9.0, OMP 18.3.1, Node 24.6.0, pinned Ubuntu image; evidence at `~/.local/state/capstan/m1-herdr-qualification/capstan-m1-herdr-gnEUjZ/qualification.jsonl`. It recorded ten acknowledgements at 9,154.613–9,480.196 ms, max progress silence 20,072.923 ms, containment 2,957.797–3,085.526 ms, and 9,256.577 ms quiescence.

The first run on `def9755` did not qualify: replacement sample 10 produced no tool-start frame before the 60-second frame deadline; the preserved pane showed the assistant still working. Cleanup completed without residual resources. The same source passed on a complete rerun, but was superseded by later peer-authorization changes. Subsequent qualification attempts exposed harness-only issues (a missing pinned peer-helper mount and reset/EPIPE handling for deliberately denied worker peers); those failed journals and diagnostics remain under `~/.local/state/capstan/m1-herdr-qualification/` (`WpoR7c`, `fmgDU8`, `vwvPmg`, `Wstgo2`, `6xNM1F`). The final source then passed 10/10/10 on the complete run above.
The earlier Docker RPC probe journals remain under `~/.local/state/capstan/m1-probe/` and are not selected-path evidence. A failed qualification retains its temporary workspace/session root for diagnosis; treat it as potentially sensitive and remove it after preserving the evidence.

Pin the tested OMP and Herdr versions in the evidence report. Earlier research observed OMP 18.3.0 and Herdr 0.9.0; those observations are not a compatibility promise for this project.

## 6. Minimum implementation contracts

### 6.1 Durable records

Keep these concepts distinct, even if several share a physical table:

- Project configuration, task brief, acceptance criteria, policy, and plan revisions.
- Parent/child work items and dependency edges.
- Stable seats, runtime sessions, and observed process identities.
- Assignments, attempts, monotonic generations, authority status, and input bindings.
- Outgoing commands, delivery attempts, acknowledgements, and deduplication receipts.
- Immutable candidates and candidate-bound verification evidence.
- Supervisor findings, deliveries, acknowledgements, corrections, and dispositions.
- Transition events, operator actions, recovery attempts, and usage observations.

Every state-changing request includes authenticated actor identity, a correlation/request ID, work and assignment identity where applicable, expected state version, and bound input revision. Reuse of an ID with different content is an error, not a successful duplicate.

Persist assignment creation and its outgoing command in one database transaction. Use at-least-once delivery with idempotent processing, not a claim of exactly-once prompt execution. The receiver durably records command identity before starting work and reconciles replay against the existing assignment rather than starting another turn blindly.

A lost acknowledgement means delivery is unknown. Query/reconcile the known command and session; do not grant a new writer merely because a timeout elapsed. If receipt/execution state cannot be established, contain the old runtime before any replacement.

### 6.2 Small explicit state machines

| Entity             | Minimum states                                                                      |
| ------------------ | ----------------------------------------------------------------------------------- |
| Work item          | pending, ready, running, awaiting_verification, accepted, blocked, canceled, failed |
| Assignment attempt | created, dispatched, acknowledged, running, reported, completed, revoked, failed    |
| Runtime session    | starting, ready, working, stopping, exited, unknown                                 |
| Finding            | detected, reported, acknowledged, correcting, resolved, disputed, escalated         |
| Run control        | active, paused, canceling, canceled, completed, failed                              |

Implement transition tables and authorization checks, not agent-authored status strings. A failed candidate can return to bounded remediation; its rejection and evidence remain recorded. A parent enters acceptance only after required children are accepted and its own final-candidate verification passes.

A paused run is not a canceled assignment. A canceled run is not proof that all processes stopped. An unknown runtime is not safe to replace until its write access is contained.

### 6.3 Assignment capsule

Send the minimum context required for the next action:

- Task objective and acceptance criteria, with source revision.
- Seat, command, assignment, attempt, generation, and current state.
- Accepted dependency candidates and accepted PM/Supervisor reports, with report receipt hash and source revision.
- Workspace, write scope, allowed operations, and policy limits.
- Relevant artifact references and unresolved findings.
- Expected result schema, completion conditions, and next legal actions.
- Recovery history needed to avoid repeating an ineffective correction.

Keep controller-held accepted facts authoritative. Summaries and worker notes are retrieval aids, not replacements for original evidence. Do not broadcast full transcripts to every seat.

### 6.4 Candidate and verification contract

A Developer report names an immutable candidate commit, its parent/base, changed scope, acceptance-related evidence, and outstanding limitations. Uncommitted work is not an accepted candidate.

The Verifier receives that exact candidate in a separate execution workspace, checks its starting identity, exercises behavior, and reports per-criterion observations with command exit status and artifact references. Generated test/build files may live in scratch space; modifying candidate source invalidates the verification attempt. After containment, the controller hashes every tracked regular file against the candidate tree and scans the actual checkout for extra files, regardless of Git ignore/index/configuration flags. The isolated runtime `.home/` and `.git/` are excluded; symlinks and gitlinks fail closed in this dependency-light MVP. Host Git reads disable replacement objects and pin the Git directory and worktree; no worker-configured status filters run after Verifier containment.

Accept the candidate only if the reported identity and current input revision match the assignment, and every passing criterion has a non-empty observation and zero exit status. Pre-migration evidence with unknown observation or exit status remains inspectable but cannot authorize a new acceptance. A new code revision invalidates old verification for the changed deliverable. The final parent check runs on the actual composed local branch tip, not merely on the last child report.

## 7. Reference task and normal workflow

Use a disposable, dependency-light JavaScript repository. The reference task is a small JSONL summarization CLI:

- **Slice A:** implement parsing and aggregation under an explicit input/output contract.
- **Slice B:** implement the command-line entry point using the accepted Slice A contract and candidate.
- **Parent acceptance:** feed a fixed JSONL file into the real executable and compare exact aggregate output; exercise malformed input, expected nonzero exit status, stderr diagnostics, and absence of misleading success output.

The actual brief must fix grouping, numeric handling, output order, and malformed-input policy before the run. The PM may propose bounded decomposition refinements; it must not invent missing acceptance semantics. Freeze the fixture and expected behavior before collecting results.

Normal sequence:

1. Operator initializes a disposable project, supplies the approved brief and limits, and starts a run.
2. Controller creates the PM assignment and activates the separate Supervisor.
3. PM returns a two-slice plan; controller validates ownership, acyclicity, scope, and dependency inputs.
4. Controller assigns Slice A to the Developer. The Developer acknowledges and implements it.
5. Controller dispatches independent verification against candidate A. A rejection routes a bounded remediation assignment; a pass accepts A.
6. Only then does the controller dispatch Slice B with accepted A as its base/input. B follows the same independent verification cycle.
7. Controller composes the accepted local candidates, proving the expected ancestry. An unexpected integration conflict blocks the run rather than triggering unplanned model-driven merge work.
8. Verifier runs the full parent acceptance scenario against the final branch tip.
9. Controller checks child dispositions, final evidence, current input revisions, and unresolved blocking Supervisor findings.
10. Controller records accepted local delivery and exports the run report. Herdr being idle never substitutes for any step.

The reference task is the first reproducible demonstration, not a special case in scheduling logic. Qualification also uses two other predeclared small tasks, including a bug fix in existing code, to avoid proving only one tailored example.

## 8. Dedicated Workflow Supervisor

### Inputs and triggers

Supply bounded event windows, current plan/assignment state, tool/error fingerprints, progress observations, handoff ages, dependency versions, findings, and remaining limits. Trigger inference on suspicious repetition, overdue handoffs, verification rejection, recovery, and significant phase transitions. Use bounded periodic evaluation only while work is active.

Deterministic monitoring detects hard state violations immediately. The Supervisor diagnoses ambiguous workflow problems and proposes targeted correction. Provider activity, tool activity, semantic progress, and waiting for a dependency/human are different signals.

### Finding lifecycle

Each finding contains an ID, affected seat/assignment/generation, issue fingerprint, severity, supporting evidence, requested correction, acknowledgement deadline, resolution condition, and escalation route.

```text
Detected -> Reported -> Acknowledged -> Correcting -> Verified resolved
                             |              |
                             +-> Disputed   +-> Escalated
```

- Report delivery, agent acknowledgement, and verified resolution are separate persisted events.
- Corrections route to the affected assignment's seat and responsible role, including the PM or Verifier, not only the Developer; another seat with the same role cannot answer the finding.
- The responsible role provides an assignment-bound acknowledgement and a correction/dispute response.
- The Supervisor checks new evidence against the stated resolution condition; a worker saying “fixed” is insufficient.
- Unresolved disputes escalate to the operator in this MVP. No silent waiver path is needed.
- Deduplicate unchanged issues, apply cooldowns, and cap interventions. A post-cooldown recurrence of the same issue records explicit provenance; distinct evidence records a new unlinked finding.
- Fingerprints bind the exact affected assignment to its latest material event and a required stable defect code (or deterministic hard-monitor inputs), not Supervisor prose or extra citations. A paraphrase of one defect retains its intervention budget; distinct coded defects on the same event remain separate. Resolved findings suppress unchanged reports only through their cooldown; reopen provenance links only to a resolved recurrence of the same fingerprint.
- A PM correction response cannot mutate the accepted active plan. The bound response is recorded, then escalated to the operator rather than treating an unchanged plan as corrected.
- Resolution proof must include the accepted correction event, exact condition, fresh Supervisor checkpoint assignment, and non-empty independent verification evidence.

The controller persists the finding lifecycle, source Supervisor assignment, affected assignment generation, evidence, disposition, correction-work binding, intervention count, cooldown, and reopen provenance. Supervisor evaluations use event-sequence-bounded snapshots (32 detailed events / 16 detailed assignments by default, plus at most 64 compact latest-material-event references for earlier assignments; payload excerpts are capped at 128 characters) and report a hard monitor for overlapping active/unknown authority on a seat. A blocked evaluation records a durable finding before containment and role-bound correction work item, then dispatches only after the fresh post-containment checkpoint and records the bound worker's acknowledgement and correction/dispute response. PM and Developer correction reports are accepted only after their corresponding reported assignment is contained.

The detailed window has at most 32 recent events plus 16 contextual latest-material events for older assignments (48 total); compact references remain capped at 64, one per assignment.

Supervisor evaluation records bind the captured event boundary and epoch to the exact assigned Supervisor generation; the controller captures the event window and authoritative snapshot in one database read transaction and rejects intervening mutations. A checkpoint cannot claim a later event boundary than its bound evaluation. Delivery and start recheck supervision after assignment, so a stale epoch cannot start ordinary work. A hard authority overlap on the Supervisor seat persists an operator-routed finding and degrades supervision without dispatching correction to that seat. An already accepted Developer slice can only be superseded by a separately verified correction based on its currently accepted candidate.

A Verifier child bound to a Developer correction candidate may deliver and start under a healthy checkpoint while the finding remains open; ordinary unrelated Verifier work remains blocked until resolution.

Only the first assigned PM work item may deliver and start before the first Supervisor evaluation or checkpoint; beginning evaluation or marking supervision degraded permanently closes this bootstrap exception. A finding report is bound to its stored source Supervisor generation, not the assignment's later active generation. Only the newest evaluation for the current epoch can checkpoint while health remains evaluating; an older accepted report cannot undo a later evaluation or degradation.

### MVP recovery defaults

These are proposed configurable experiment defaults, not proven production thresholds:

- Three equivalent failures with unchanged relevant inputs trigger evaluation, not automatic termination.
- At most two corrective interventions for the same unresolved issue, then block/escalate.
- At most one automatic worker replacement per slice, after containment is proven.
- At most two implementation-remediation cycles after the initial verification rejection, then block/escalate.
- Set acknowledgement, progress-evaluation, and run-duration limits from the runtime probe before measured runs; freeze them in the run policy. A long legitimate tool invocation gets a tool-specific window.
- Configure a total token/cost limit where the provider supplies usable accounting, plus a wall-clock and dispatch/intervention limit that does not depend on token reporting. Unknown usage is never treated as zero.

If the Supervisor fails or an evaluation exceeds its deadline, show degraded supervision, stop new assignments, and withhold acceptance. Already running work may stop at its current bounded action while the controller attempts one Supervisor replacement. If supervision cannot be restored, pause the run and escalate. Never mark the unsupervised interval as supervised.

## 9. Recovery, isolation, and operator control

### Safe replacement

1. Pause dispatch for the affected work and persist the recovery reason.
2. Revoke the old assignment generation; reject subsequent state-changing reports from it.
3. Interrupt and contain the old runtime, including child processes capable of writing. An expired lease or revoked database token does not stop filesystem writes.
4. Preserve workspace changes, logs, and the last accepted candidate. Do not delete user artifacts during recovery.
5. Prove old write authority has ended. If that cannot be proven, remain blocked; do not launch another writer into the same surface.
6. Start a new runtime/generation with a reconstructed capsule and a recorded recovery action.
7. Independently verify any recovered candidate before acceptance.

On controller restart, obtain exclusive controller ownership, reconcile runtime identities and outbox receipts, rebuild pending responsibility, and only then resume dispatch. Two controller processes must not both schedule work for the same project.

### Trust boundary

Run the MVP in a disposable environment with no production repository, tracker, Git-push, cloud, or deployment credentials. Explicitly approve the model-provider access required for OMP. Do not inherit the operator's general home directory, SSH agent, or unrelated credentials into worker environments.

Use per-role workspaces and controller API credentials. PM, Verifier, and Supervisor receive no controller-authorized source-writing or delivery capability. Select and exercise the actual process/container/VM containment mechanism during feasibility; if it cannot protect controller state and contain a replaced writer, the recovery gate fails. Do not assume Podman or any particular runtime is installed.

Worktrees and OMP profiles alone are not security boundaries. Prompt restrictions and tool hooks alone do not establish containment. The MVP qualifies its tested operational boundary, not resistance to arbitrary malicious code, provider compromise, or all network exfiltration paths.

### CLI commands and contracts

`cstan init`, `cstan run --brief`, `cstan status`, `cstan inspect`, `cstan pause`, `cstan resume`, and `cstan cancel` are executable. `report` remains a proposal.

| Command                        | Purpose                                                                                  |
| ------------------------------ | ---------------------------------------------------------------------------------------- |
| `cstan init`                   | Establish project-local configuration and state location                                 |
| `cstan run --brief <file>`     | Validate an approved brief and start the bounded delivery loop                           |
| `cstan status [--json]`        | Show run state, role health, ownership, blockers, limits, and next actions               |
| `cstan inspect <id> [--json]`  | Inspect a task, assignment, candidate, finding, or recovery                              |
| `cstan pause`                  | Stop new dispatch; expose still-running work without claiming it stopped                 |
| `cstan resume`                 | Reconcile state/authority and resume a paused run                                        |
| `cstan cancel`                 | Revoke assignments, contain runtimes, and retain evidence; report incomplete containment |
| `cstan report --output <path>` | Export candidate identity, acceptance results, findings, recovery trace, and usage       |

The controller runs in the foreground under `cstan run`; `status` and `inspect` use authenticated local IPC while it is active and reopen the durable database read-only after it exits. `pause`, `resume`, and `cancel` require the active authenticated foreground control socket; after a controller crash, restart with the same `run --brief <file>` to acquire ownership and reconcile before using operator controls. Offline inspection never acquires ownership, migrates, or reconciles uncertain commands. Pausing withholds new dispatch but does not stop active work. Cancel stays `canceling` if containment cannot be proven and retains workspaces, receipts, and evidence. Exit statuses: `0` complete/success, `2` usage, `3` invalid input or project configuration, `4` blocked or incomplete bounded work, `5` runtime or containment failure.

Live `resume` inspects uncertain commands and current runtime authority without invoking crash reconciliation on workers left running by `pause`; unresolved uncertainty keeps the run paused. `cancel` waits for in-flight provisioning and only reports `canceled` after registered and newly provisioned runtimes are physically contained; an incomplete proof stays `canceling`. A subsequent `run --brief` retries exact persisted runtime containment for a `canceling` run, then marks it `canceled` without resuming dispatch; missing physical evidence keeps it blocked. Restart counts persisted runtime sessions and progressed work against `maxDispatches` without inventing a dispatch before any session existed.

Before `cstan run`, set `M1_PROVIDER_HOST` to the exact egress-allowlisted provider hostname and optionally `M1_PROVIDER_PORT` (default `443`); set absolute paths in `M1_HERDR_BINARY`, `M1_OMP_BINARY`, `M1_OMP_NATIVE_ADDON`, and `M1_NODE_BINARY`. The host Git executable is resolved from `PATH` and its exec directory is mounted read-only into role containers so Developer sessions can create immutable commits in their writable assignment checkout. Developer checkouts inherit the project Git `user.name` and `user.email` locally; a missing identity fails before dispatch because candidate commits cannot be produced without it. Preflight enforces Herdr `0.9.0` and its pinned SHA-256, OMP `18.3.1`, Node `24.6.0`, the pinned container image, Docker/cgroup-v2 isolation, compiler, and firewall/egress controls. Missing prerequisites fail before controller database creation; no host-execution fallback exists. An existing database is reopened only under the controller lock and reconciled before dispatch; missing runtime proof or scheduler identity blocks rather than blindly redispatching.
Assignment capsules identify the workspace as the role container sees it (`/workspace`); this maps to the isolated per-assignment checkout mounted by the controller.
The CLI resolves qualification helper scripts relative to the installed Capstan package, not the project working directory.
Before provisioning role checkouts, `run` requires the project directory to be the Git repository root and rejects any root or nested `.capstan` entry or descendant in reachable Git history or the current index; cloned workers must never receive the operator key or controller state.

All JSON envelopes use `schemaVersion: 1`. `run --brief` emits `{state, projectId, planHash, baseSha, roles, scheduler, blocker?}`. `status --json` emits `ControllerStatus` (`projectId`, `run`, `stateVersion`, `inputRevision`, `roles`, `work`, `findings`, `evidence`, `finalVerification`) plus `ownership`, `blockers`, `limits`, and `nextLegalActions`. `finalVerification` contains the exact composed commit and criterion-level artifact references after contained parent acceptance. `inspect --json` emits `{schemaVersion, kind, id, record}`. The runtime TypeScript contracts are `CstanRunJsonV1`, `CstanStatusJsonV1`, and `CstanInspectJsonV1` in `src/cli.ts`; executable contract tests exercise the output envelopes and exit codes.
Work-item `nextLegalActions` includes `mark_ready` only when the controller’s readiness checks pass; eligible pending work is not reported as waiting.
Ready work advertises `assign` only while the controller's current readiness gate permits assignment. Validated dispatch limits reserve one PM, one Developer and one Verifier per slice, one final-parent Verifier, and one Supervisor dispatch.
Authenticated control requests remain capped at 16 KiB; responses are capped at 1 MiB so large inspected records fit the same bounded local IPC contract.
Brief and project JSON are decoded as strict UTF-8 (a leading UTF-8 BOM is accepted); duplicate member names are rejected before validation. Candidate filename comparison uses Git's NUL-delimited output, including non-ASCII names; plan write scopes reject the unbounded project-root `.` scope.
Host-side Git validation starts only after the Developer runtime is contained and disables worker-configured fsmonitor, hooks, clean/process filters, external diff, and text conversion; assignment clone/checkout runs with isolated Git configuration, no initial checkout, and nonlocal object transfer. The clone never copies unreachable local credential blobs or executes configured filters on the host. Terminal cancellation records the legal `canceling` then `canceled` transition, keeps signal handlers installed through runtime-manager closure, and never reports success after a signal.
After individual slice acceptance, an independent Verifier tests the exact composed checkout HEAD against every parent criterion. Its contained assignment, exact commit, receipt-bound results, and per-criterion artifacts are recorded before final acceptance. The Supervisor's assignment depends on the accepted PM report, every accepted slice, and final-parent verification; its capsule exposes candidate-bound artifacts and the final report. It reviews the same composed checkout and read-only Verifier evidence mounted under `/evidence`; a structured blocked observation persists as a finding and withholds successful run completion. Evidence files are inspected only after Verifier containment. Run completion remains bounded by `maxRunMs`, including final verification, supervision, and cleanup. Runtime provisioning fails closed if the expected OMP host PID cannot be bound.
PM-9 restart reuses the project lock, persisted role identities, assignment-bound context keys, and controller outbox. Queued pre-attempt commands remain visible as unknown responsibility; attempted commands are never blindly resent. Each persisted runtime identity must be contained through its exact Docker container and previously observed cgroup before any fresh writer is provisioned. A vanished cgroup is acceptable only after Docker confirms that exact container stopped or is absent; a still-present cgroup must have no members. The manager then proves Docker absence, egress policy and network removal, and the pinned bridge socket identity. An unverified identity or missing context history blocks dispatch. Recovery records preserve reason and immutable evidence; replacement consumes the per-work-item budget and reconstructs the capsule from controller-held accepted facts. Missing provider accounting is recorded as unavailable without a numeric zero. DEC-003's independent same-user Herdr caller and DEC-004's lock-path tampering remain outside the accepted guarantees.

The first PM assignment may be replaced after a crash before the first Supervisor checkpoint only when its prior authority is proved contained and a bounded pending recovery record authorizes the replacement. Restart reuses pending recovery identities after an interrupted handoff and allocates new runtime session ordinals beyond persisted metadata. Persisted runtime metadata without a matching database session is contained before dispatch; an assignment with no runtime identity cannot be declared contained merely because no metadata exists. Already-absent exact containers are rechecked against saved cgroup, egress, network, and socket identities; unavailable exit status and kill history remain `null`. The controller rebuilds the role bridge journal from durable receipts with a containment boundary only after the old assignment has a physical containment proof; the old command remains unknown, while the new bridge releases its old active slot for a distinct assignment. A prestart command with attempted delivery requires a verified same-command bridge snapshot before destroying that bridge; if inspection is unavailable, the runtime is contained but dispatch remains blocked rather than inferring that the request never started. Supervisor context keeps the latest detailed events within 24 KiB while retaining durable event references, assignment handoff ages, candidate evidence, and final verification results.

Each Developer candidate report binds its assignment, generation, current input revision, commit/base SHAs, changed scope, limitations, and nonempty implementation-evidence strings into an immutable report hash. Verifier evidence is accepted only for the exact candidate checkout, complete criterion set, observations, exit statuses, and contained artifact files; rejected evidence is retained while bounded remediation creates a fresh candidate and verification generation. Accepted slices compose deterministically in validated topological order; a non-fast-forward merge conflict blocks without model-driven resolution. Final-parent verification must preserve the composed starting SHA, report the same current HEAD after read-only checks, and pass every parent criterion before controller acceptance.

Report polling is bound to the active assignment ID, not merely the work-item ID; late reports from a rejected generation remain historical and cannot satisfy a replacement assignment's wait.

Criterion evidence records retain the Verifier's observation and reported exit status alongside pass/fail and artifact identity. A passing result requires reported exit status zero; nonzero checks cannot authorize acceptance. Final-parent receipt binding also checks the same observation and status before recording the composed-tip result. These observations, statuses, and free-form artifact contents come from the independent Verifier role, not a host-attested command runner: the controller checks identity, containment, complete criterion coverage, and artifact location/nonemptiness but cannot prove that an arbitrary command ran or that a free-form log agrees with a reported status. This is a Verifier trust boundary, not protection against a dishonest Verifier. The frozen JSONL task receives a separate host-orchestrated replay in an offline container after composed-tip acceptance.

Migration-added report fields remain `null` for pre-migration records rather than manufacturing observations or recomputing their immutable historical hashes. New reports must supply the fields; consumers can distinguish legacy evidence from the current report shape.

Run `node scripts/pm7-reference-run.mjs` after `npm run build` to exercise the frozen `jsonl-summary-feature` M0 fixture in a private disposable repository. The complete fixture manifest is an immutable file in the base commit and pinned by SHA-256 so each Verifier can access the exact oracle. The runner clears inherited `GIT_*` variables and Node preload paths, materializes each case into a fresh directory with distinct HOME, XDG config/cache/data, and temporary directories from accepted tracked files without cloning a worker-controlled repository, and executes the frozen cases on the local Docker socket using a clean CLI config and the pinned local Ubuntu image. Each container has no network, a read-only root, dropped capabilities, and only the case directory plus Node executable mounted. The required image must already be present locally; the runner does not pull it. It checks valid, malformed, and regression cases byte-for-byte.

## 10. Implementation stages and exit gates

These are Capstan implementation milestones, not claw8 runtime phases. Each gate requires stored evidence before dependent work proceeds.

| Stage                          | Work                                                                                                                                                                                                                                                                                                                                       | Exit gate                                                                                                                                                                                                                                                                  |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| M0 — Freeze experiment         | Approve bounded scope, reference brief, limits, evidence format, safety environment, and baseline method. Inspect decisive OpenRig seams.                                                                                                                                                                                                  | Record foundation choice and all run inputs; no unresolved authority or delivery target.                                                                                                                                                                                   |
| M1 — Prove runtime feasibility | Exercise Herdr-hosted OMP delivery, role identity, command-bound receipts/replay, interruption, reconnect, scoped egress and writer containment with the selected bridge. Separately test close-before-restore and the earlier controller-process crash using real OMP RPC workers; that RPC path is diagnostic, not the selected runtime. | Record DEC-003's accepted independent-caller bypass and the frozen selected-path 10/10/10 qualification in Section 5.2. Do not conflate receipt-listener restart and fresh-client replay with a selected controller-process crash; full controller recovery belongs to M3. |
| M2 — Durable vertical slice    | Implement controller records/transitions, outbox, role capabilities, context capsules, minimal CLI, and the four-seat normal loop.                                                                                                                                                                                                         | Reference task reaches independently verified local delivery with dependency gating and Supervisor active. No mocked role outputs in the demonstration.                                                                                                                    |
| M3 — Supervision and recovery  | Implement findings, correction/dispute routing, bounded recovery, restart reconciliation, stale rejection, and operator controls.                                                                                                                                                                                                          | All reliability and negative-path scenarios in Section 11 pass against real runtime sessions.                                                                                                                                                                              |
| M4 — Qualification             | Run the fixed task set and comparison baseline; export evidence and review limitations.                                                                                                                                                                                                                                                    | Issue a correctness go/no-go decision and a separate efficiency verdict under Section 12.                                                                                                                                                                                  |

Build one vertical path before generalizing. Keep runtime-adapter, persistence, state-transition, and role-output boundaries explicit, but do not implement a generic agent platform to obtain this proof.

## 11. Acceptance and failure-injection scenarios

Run failures at named boundaries using a harness that records the injection and its timestamp. Do not manufacture passing evidence with canned agent replies. Test fixtures may deliberately cause failures; the actual controller, bridge, OMP sessions, Supervisor, and verification path must handle them.

| ID  | Scenario                                    | Required observable outcome                                                                                                                                                                                                                                                        | Hypotheses |
| --- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
| A01 | Normal two-slice delivery                   | Distinct role sessions; B waits for accepted A; exact final-parent behavior passes; local candidate and evidence are recorded.                                                                                                                                                     | H1, H5     |
| A02 | Incorrect candidate                         | Independent Verifier rejects wrong behavior; remediation creates a new candidate; acceptance uses fresh verification, not old evidence.                                                                                                                                            | H5         |
| A03 | Repetitive Developer failure                | Inject a repeated deterministic tool failure with an actionable correction. Supervisor routes evidence-backed guidance; agent acknowledges; condition is independently checked as resolved, or bounded escalation occurs. At least one run must demonstrate successful correction. | H3         |
| A04 | Coordination issue outside Developer        | Delay a required PM response after acknowledged delivery. Supervisor reports to the PM, tracks correction, and verifies resumed responsibility.                                                                                                                                    | H1, H3     |
| A05 | Legitimate long-running work                | Exercise a tool within its declared long-running window. No destructive replacement or false resolution solely because files are unchanged.                                                                                                                                        | H3         |
| A06 | Controller crash before dispatch            | Crash after assignment/outbox commit and before send. Restart delivers existing work without loss or a second assignment.                                                                                                                                                          | H2, H4     |
| A07 | Lost acknowledgement                        | Drop acknowledgement after the worker records receipt. Reconciliation/replay does not start a second execution; uncertainty remains visible until resolved.                                                                                                                        | H4         |
| A08 | Worker crash and late report                | Preserve partial work, contain the old process tree, start a new generation, and inject an old-generation report. Reject it; prove no overlapping authorized writers.                                                                                                              | H2, H4     |
| A09 | Supervisor failure                          | Show degraded supervision, prevent new dispatch/acceptance, restore a separate Supervisor within policy or pause/escalate.                                                                                                                                                         | H3, H4     |
| A10 | Repeated unsuccessful recovery              | Hit the configured intervention/replacement limits. Park with evidence and a precise decision request; no spawn or retry loop.                                                                                                                                                     | H3, H4     |
| A11 | Pause, resume, and cancel                   | Paused dispatch stays stopped; resume reconciles existing work; cancel reports containment truthfully and retains evidence.                                                                                                                                                        | H2, H4     |
| A12 | Stale inputs and duplicate reports          | Change an accepted brief/dependency revision through the controlled update path. Block or invalidate affected work/evidence; exact report replay is idempotent; conflicting ID reuse fails.                                                                                        | H2, H4, H5 |
| A13 | Duplicate controller / capability violation | Reject a second scheduling owner; reject Developer self-acceptance and Supervisor direct assignment changes; prevent worker writes to controller state under the selected boundary.                                                                                                | H4, H5     |
| A14 | Clean session replacement                   | Reconstruct the objective, accepted inputs, open findings, and next legal actions without the old conversation; complete the remaining work.                                                                                                                                       | H2         |
| A15 | Matched baseline comparison                 | Export all role/controller-associated usage that is measurable, elapsed time, quality, and human interventions; mark missing usage and inference separately.                                                                                                                       | H6         |

For A12, a controlled update can be a narrow operator/controller API rather than an additional public CLI command. Direct database editing is not a valid input-change demonstration.

Use deterministic regression tests for transition authorization, dependency readiness, idempotency, candidate binding, and recovery limits. Keep failure-injection integration coverage for the real adapter. Tests alone do not replace the runtime demonstrations above.

## 12. Measurement and go/no-go decision

### Evidence bundle

Each run records:

- Source/task fixture revision, accepted brief and plan revisions, configured limits, and environment/version manifest.
- Model/provider identity and role settings, with unavailable fields explicitly marked unknown.
- Seat/session/assignment/generation identities and an ordered state/event trace.
- Dispatch and acknowledgement receipts, failure-injection markers, and recovery decisions.
- Candidate identities and per-criterion independent verification output.
- Supervisor findings, addressed role, acknowledgements, corrections, and resolution evidence.
- Token usage by role including Supervisor and any recovery sessions; provider cost where available; missing observations separately.
- Wall-clock duration, human interventions, repeated-action episodes, and retry/replacement counts.
- Final delivery status, blocking findings, and limitations. Do not export credentials.

### Qualification set

Use three frozen tasks: the reference feature, a bounded bug fix, and another small feature in existing code. Run each three times from clean starting snapshots for nine Capstan normal-workload runs. Apply the same fixture revisions, acceptance criteria, and comparable model/provider settings to nine claw8 baseline runs. Record unavoidable configuration differences instead of attributing every difference to orchestration.

Run each deterministic fault scenario A06–A14 at least once, including the necessary subcases; run A03–A05 three times each because they involve Supervisor judgment. A02 must demonstrate rejection and correction at least once. Record all failures, not only successful reruns. This is a feasibility sample, not a statistical reliability guarantee.

If a matched claw8 baseline cannot be exercised, complete correctness qualification but label efficiency comparison **not established**. Do not block safety findings or claim token savings from incomplete accounting.

### Hard correctness gate

A **go for expanding the product** requires:

- All A01–A14 scenarios pass their stated outcomes, with evidence from the selected runtime path.
- At least eight of nine normal-workload runs reach correct local delivery without manual workflow routing; any remaining run must stop safely with a precise blocker rather than falsely succeed.
- Zero observed duplicate write ownership, stale-result acceptance, lost committed handoffs, false parent acceptance, or unauthorized controller transitions.
- At least one Supervisor-led loop correction succeeds end to end, and unresolved cases respect their bounds.
- Recovery survives actual controller/runtime interruption; simulated persistence tests alone are insufficient.

A safety invariant violation is a **no-go**, even if the delivery-rate target is met. Fix the failing invariant and rerun the affected scenarios before expanding scope.

### Separate efficiency verdict

Report total tokens/cost per accepted task, elapsed time, and interventions for both systems, including failed-run expenditure. Keep end-to-end results alongside per-role costs so Supervisor/PM overhead is visible.

- **Efficiency demonstrated:** complete comparable accounting shows lower token/cost expenditure or fewer human interventions without reduced observed acceptance quality; identify which metric improved and any regressions.
- **Correctness feasible, efficiency inconclusive:** safety gates pass, but the sample, accounting, or differences do not support a savings claim.
- **Efficiency regression:** coordination overhead or interventions increase without a compensating measured outcome; revise activation/context policy before claiming the product addresses efficiency.

These verdicts apply only to the tested workload. A correctness go may justify the next bounded stage even when token savings are not established; it does not justify marketing an efficiency improvement.

## 13. Deliverables and next decision

The MVP deliverable is working source plus:

1. A usable `cstan` executable implementing the approved MVP commands.
2. A pinned, exercised OMP/Herdr runtime integration and four separate role configurations.
3. Durable controller state, replay-safe delivery, independent verification, and mandatory supervision.
4. Reproducible task fixtures and failure-injection scenarios, with the behavioral regression coverage described above.
5. Run evidence and a go/no-go report, including negative outcomes and the separate efficiency verdict.
6. Operator instructions for starting, inspecting, pausing, canceling, recovering, and preserving evidence, with explicit environment and security limits.

If the gate passes, the next bounded expansion is the full role contracts and knowledge lifecycle, followed by concurrent isolated slices and controlled integration. Do not jump directly to unrestricted production repositories, external credentials, automated merging, or deployment.

**Next action:** review this MVP boundary, then begin M0 in the Capstan project. Resolve the foundation/runtime and containment questions before building the full controller. The immediate goal is evidence that the coordination contract works—not the appearance of a busy multi-agent team.

## 14. References and evidence limits

This document is self-contained for MVP implementation planning. It derives from the broader “Capstan — Autonomous Software Delivery Framework: Product Goals, Requirements, and Architecture Plan,” specifically its feasibility/durable-coordination stages, independent role contracts, mandatory Supervisor, and recovery invariants.

Primary upstream references:

- [OMP](https://github.com/can1357/oh-my-pi), [extension API](https://github.com/can1357/oh-my-pi/blob/main/docs/extensions.md), and [session persistence](https://github.com/can1357/oh-my-pi/blob/main/docs/session.md).
- [Herdr agent automation](https://herdr.dev/docs/agent-automation/), [socket API](https://herdr.dev/docs/socket-api/), and [session state](https://herdr.dev/docs/session-state/).
- [OpenRig](https://github.com/mvschwarz/openrig).
- [Existing OSv Capstan command](https://osv.io/capstan).

Upstream documentation is mutable. Runtime capabilities, security boundaries, savings, and acceptance outcomes in this plan remain to be demonstrated; creating this document does not establish any of them.
