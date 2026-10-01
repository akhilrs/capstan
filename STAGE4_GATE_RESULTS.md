# Stage 4 gate: reviewer and integration (PM-32)

This file records the Stage 4 gate in two parts. Part 1 (the reviewer) is below. Part 2 (integration) is added by its own PR.

## Part 1: independent review

Run on 2026-10-01 in a throwaway repository with an isolated Herdr session (`capstan-s4`). Every agent was the fake Claude stand-in, not real Claude, so no usage was spent. Each agent's token was read from `/proc/<pid>/environ` of its own process (same user) and used from a separate shell. The evidence is in `docs/stage4-gate-evidence.txt`; tokens are not in it.

### What ran

1. `developer-1` committed `feature.txt` in its worktree and reported the commit (`cstan report`, accepted).
2. The PM asked for a review (`cstan request-review <report-id>`). The controller spawned `reviewer-1`, a new agent of the `reviewer` role (a Verifier role), on a worktree and branch at the reported commit (`capstan/reviewer-1-g1`, listed by `git worktree list` while the review ran).
3. The author's own token tried `cstan review pass …` and was refused: `forbidden: only a reviewer can answer a review`.
4. `reviewer-1` answered `cstan review findings "…"`. The ledger recorded round 1 as `findings`; the PM notice was queued (sender: the controller's internal actor). `reviewer-1` was released: its agent ended, its pane closed, its worktree removed and its branch deleted.
5. The developer committed a test and reported the new commit; the PM asked for a review of that report; the controller spawned `reviewer-2`, a different agent with a different actor. `reviewer-2` answered `pass`, and was released the same way.

### Gate oracle

| Check                                                                  | Result                                                                                                                                                                                                                                                                                                                   |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Independent review, findings, fix and re-review                        | PASS: round 1 `findings`, then (after a fix and a new verified report) a review of the fix `passed`                                                                                                                                                                                                                      |
| The reviewer is never the author's session (session ids in the ledger) | PASS: both reviews record `author_agent_id = developer-1`, the author actor `478a4be7…`, and reviewers `reviewer-1` (actor `a86a601f…`) and `reviewer-2` (actor `8be04cf4…`); the ledger refuses a row where the reviewer's actor or agent equals the author's (a CHECK, tested), and the core refuses it before writing |
| Worktrees and review branches are removed afterwards                   | PASS for reviews: after each verdict only the author's worktree and branch remained (`git worktree list` and `git branch --list 'capstan/*'` in the evidence file)                                                                                                                                                       |
| An integration conflict blocks and is reported                         | Part 2                                                                                                                                                                                                                                                                                                                   |

### Limits and what this run does not show

- The reviewer was the stand-in: it did not read the diff or judge anything. The run shows the controller's mechanics (spawn at the commit, the task message, the verdict, the notice, independence, cleanup), not review quality, and not that real Claude follows the reviewer prompt.
- The reviewer's read-only profile is tool rules plus prompt: the starter `reviewer` role denies Write, Edit, NotebookEdit, Agent, Task and git push (a Verifier role without its own `deny` list has none), but a reviewer can still run git in its own worktree. It is not a sandbox.
- Independence means a different agent and actor (a new session with a new token). The author could still, with another agent's token, answer as that agent; this is the same-user hole of DEC-005.
- A reviewer that never answers keeps its review `started` and its slot until the PM releases it (which cancels the review); liveness detection is Stage 6. A reviewer spawned but not recorded because the daemon died in that window stays visible in the agent list until released.
- The re-review in this run was a review of the fixing report (round 1 of that report). Rounds above 1 for the same report occur when an earlier round was cancelled or failed; they are covered by tests.
