# Stage 5 gate: supervisor and findings routing (PM-33)

Run on 2026-10-01 in a throwaway repository with an isolated Herdr session (`capstan-s5`). Every agent was the fake Claude stand-in, so no usage was spent; the default Herdr session was compared before and after and did not change. `capstan.toml` had `[timers] finding_check_seconds = 60` so the deadline run is short. The evidence is in `docs/stage5-gate-evidence.txt`; tokens are not in it.

## What ran

The "repeated failing tool call" is simulated: the stand-in runs no tools, so the same failing line was typed into the developer's pane several times and appears on its screen as repeated lines. The Supervisor read them through the controller, not through Herdr.

1. **Observe.** `supervisor-1` (a role of kind `Supervisor`, spawned with `cstan spawn supervisor`) ran `cstan observe developer-1` and got the developer's screen with four identical `RUN npm test -> FAIL: expected 3 got 4 ...` lines, the agent's Herdr state and the note that the text is the agent's own screen, not verified. A developer's token running `cstan observe` and the Supervisor's token running `cstan spawn` were both refused (`forbidden`).
2. **Resolved path (finding 1).** The Supervisor raised a `high` finding quoting the repeated lines, a correction and a done-when condition. The controller wrote the finding, the delivery and a PM notice together and queued a message from its own actor to `developer-1` (the three fields JSON-quoted and labelled as the Supervisor's words). A `finding check` before the ack was refused. The developer acknowledged the message; the Supervisor observed a passing run and recorded `check ... resolved` with that evidence; the finding ended `resolved` and the PM got `raised` and `resolved` notices.
3. **Two interventions and escalation (finding 2).** A second finding about the same developer (allowed once the first closed): the developer acked, the Supervisor recorded `unresolved` with new evidence, and the controller sent the second correction ("intervention 2 of 2", quoting the new evidence, the original correction and done-when). A check before the second ack was refused. After the second ack the Supervisor recorded `unresolved` again: the finding ended `escalated` (`second_unresolved`), the PM got an `ESCALATED to the operator` notice, and a third check was refused (`the finding is already escalated`). No third correction exists: the deliveries table has two rows for it.
4. **Deadline (finding 3).** A finding about `developer-2` that nobody checked: 75 seconds after the raise the controller ended it `escalated` (`timed_out`) with a check of its own (`timed_out`), cancelled the undelivered correction and told the PM.
5. **Operator view.** `cstan status --watch` listed both escalated findings as `needs the operator` (and rings the bell once per escalation).

## Gate oracle

| Check                                                                        | Result                                                                                                                                                       |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A finding exists that cites the repeated outputs as evidence                 | PASS: findings 1 and 2 quote the repeated failing lines (ledger table `agent_findings`)                                                                      |
| It is delivered to the developer and acked                                   | PASS: the delivery messages are from the controller's actor to `developer-1`, state `acked`                                                                  |
| A resolution check is recorded                                               | PASS: `agent_finding_checks` has `resolved` (finding 1), `unresolved` twice (finding 2) and the controller's `timed_out` (finding 3), each with its evidence |
| Or, after two correction attempts, an escalation to the operator is recorded | PASS: finding 2 ended `escalated` after the second unresolved check; the ledger, the PM notice and the operator's watch view show it                         |
| A third correction is never made                                             | PASS: two delivery rows only; the third check was refused                                                                                                    |

## Limits and what this run does not show

- The Supervisor in this run was driven by me with its own token. The run shows the controller's mechanics (observe, validation, delivery, ack, checks, the two-intervention limit, the deadline, notices); it does not show that a real Supervisor notices a loop by itself, writes good evidence, or that a real developer changes course after a message.
- A looping agent usually shows as `working`, and the messaging layer sends nothing into a working pane: the correction stays deferred, may expire, and the finding then escalates at the deadline with nothing delivered. That outcome is visible but does not interrupt the loop; an interrupting delivery is a follow-up.
- The text a Supervisor writes reaches the target through the controller's actor. It is JSON-quoted and labelled as data, which lowers but cannot remove the risk that a model follows instructions inside it.
- The Supervisor's read-only profile is tool rules plus prompt (Write, Edit, NotebookEdit, Agent, Task, `git push` and `herdr` denied in the starter role); it is not a sandbox, and Bash stays open for `cstan`. Independence of the Supervisor from the workers is by convention only.
- The observation is a screen reading (the visible 40 to 120 lines), not a tool trace. Observation and finding rates are counted in memory, so a daemon restart resets them.
- Auto-start of the Supervisor is a follow-up: the PM or the operator spawns it (`cstan spawn supervisor`).
- Covered by tests, not by the live run: refusals of every wrong caller and target, text rules, replaced generations of the raiser and the target, cancel on end, notices with no PM or several, ledger triggers, the sanitizer and the migration.
