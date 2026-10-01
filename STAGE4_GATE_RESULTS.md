# Stage 4 gate: reviewer and integration (PM-32)

This file records the Stage 4 gate in two parts: part 1 (the reviewer, PM-32) and part 2 (integration, PM-36).

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
| An integration conflict blocks and is reported                         | PASS in part 2                                                                                                                                                                                                                                                                                                           |

### Limits and what this run does not show

- The reviewer was the stand-in: it did not read the diff or judge anything. The run shows the controller's mechanics (spawn at the commit, the task message, the verdict, the notice, independence, cleanup), not review quality, and not that real Claude follows the reviewer prompt.
- The reviewer's read-only profile is tool rules plus prompt: the starter `reviewer` role denies Write, Edit, NotebookEdit, Agent, Task and git push (a Verifier role without its own `deny` list has none), but `Bash(git *)` still lets a reviewer run git against any worktree, the author's included. It is not a sandbox.
- Independence means a different agent and actor (a new session with a new token). The author could still, with another agent's token, answer as that agent; this is the same-user hole of DEC-005.
- A reviewer that never answers keeps its review `started` and its slot until the PM releases it (which cancels the review); liveness detection is Stage 6. A reviewer spawned but not recorded because the daemon died in that window stays visible in the agent list until released.
- The re-review in this run was a review of the fixing report (round 1 of that report). Rounds above 1 for the same report occur when an earlier round was cancelled or failed; they are covered by tests.

## Part 2: integration

Run on 2026-10-01 in a throwaway repository with an isolated Herdr session (`capstan-s4b`). Every agent was the fake Claude stand-in, so no usage was spent; the default Herdr session was compared before and after and did not change. The evidence is in `docs/stage4-gate-evidence.txt` (part 2); tokens are not in it.

### What ran

1. Two developers (`developer-1`, `developer-2`) each committed a different new file and reported it. The PM asked for a review of each report; two new reviewers (`reviewer-1`, `reviewer-2`) answered `pass` and were released.
2. The PM ran `cstan integrate <report-1> <report-2>`. The controller recorded the base (the project's HEAD, `f6ead93`), merged both commits in that order as two merge commits (author `capstan`) and created the branch `capstan/integration/<id>` at `1fa5e77`. No worktree was created for it (`git worktree list` showed only the repository and the two authors' worktrees).
3. `confirm` before the branch was in HEAD was refused (`not_in_head`). The PM asked for a review of the integration (`cstan request-review <integration-id>`). A new reviewer (`reviewer-3`, a different agent and actor from both authors) was spawned at the merged commit, answered `pass`, and was released (worktree and branch gone). The author's own token tried `cstan review` and was refused (`forbidden`).
4. The branch was merged into HEAD (a fast-forward by the operator) and `cstan integrate confirm <integration-id>` ended the integration as `confirmed` and removed the branch (`branchRemoved: true`).
5. Each developer then committed a second change to the same line of `shared.txt`; both were reported and passed review (`reviewer-4`, `reviewer-5`). The PM ran `cstan integrate` with both. The merge stopped at the second report: the answer was `state: conflicted`, the report id and `files: shared.txt`; the ledger recorded the same; no branch and no worktree stayed and `git status` showed no change to the project. The operator then ran the same integration: the result was `conflicted` again and the PM received a message from the controller naming the integration, the report and the file, and saying the controller does not resolve conflicts.

### Gate oracle

| Check                                                                | Result                                                                                                                                                                                                                                                      |
| -------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| An integration conflict blocks and is reported                       | PASS: the integration ended `conflicted` with the report and file recorded; the PM got the answer (PM caller) and a controller message (operator caller); nothing was merged                                                                                |
| Worktrees and review branches are removed afterwards                 | PASS: after the integration review and after the conflict, only the authors' worktrees and branches remained; the integration branch was removed on confirm and was never created on a conflict; every reviewer's worktree and branch was gone              |
| The integrated commit is reviewed by a reviewer who is not an author | PASS: the review of the merge records subject kind integration; `reviewer-3` (actor `d8329cbb…`) is neither `developer-1` nor `developer-2`; the ledger refuses a reviewer who authored any merged report (a trigger, tested) and the core refuses it first |

### Limits and what this run does not show

- The stand-in agents did not read or write any real code; the run shows the controller's mechanics, not review quality or that real Claude follows the PM and reviewer prompts about `integrate`.
- The merge is done with `git merge-tree` and `git commit-tree` and nothing is checked out, so no hook, filter, fsmonitor, signing or rerere setting of the repository is used. A merge driver named in committed attributes and defined in the repository configuration can still run (DEC-005's same-user hole). The result also depends on the repository's merge and line-ending settings and attributes.
- A developer resolving a conflict "as a new candidate" is the PM's workflow (assign, report, review, integrate again). This run shows that the conflict blocks and is reported, not a full resolution round.
- `integrate confirm` needs the merged commit to be in HEAD already; the controller never merges into the project's HEAD or pushes. A branch that could not be deleted at settlement stays until the next integration or daemon start sweeps it; it never blocks anything.
- The same cut-off cases are covered by tests (`test/integration.test.ts`, `test/integration-git.test.ts`), not by the live run: a daemon stop while merging, an outcome that cannot be recorded, a checked-out branch, hostile repository settings and conflict path escaping.
