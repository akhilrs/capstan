# Stage 3 gate: facts are verified, not trusted (PM-31)

Run on 2026-10-01 against the Capstan build at the PM-31 branch, in a throwaway repository under `/tmp/capstan-s3` with an isolated Herdr session (`capstan-s3`). The worker process was the fake Claude stand-in, not real Claude: no usage was spent. The evidence is in `docs/stage3-gate-evidence.txt` (ledger rows, the PM notice, git's view of each commit and `cstan inspect` output). The token is not in that file.

## Setup

`cstan start`, then `cstan spawn developer`. The worker `developer-1` got the branch `capstan/developer-1-g1` at the base commit `ebbf59ea31e7`. The worker's token was read from `/proc/17297/environ`, the environment of the worker's own process (same user, no privilege), and used from a separate shell through `CAPSTAN_TOKEN` and `CAPSTAN_SOCKET`. That shell is the forger in both cases.

## Case (a): forged reports for commits that are not on the agent's branch are rejected

| Forged report                                     | Result                        | Ledger   |
| ------------------------------------------------- | ----------------------------- | -------- |
| a commit that exists only on `main` (`415f2699…`) | rejected, `not_on_branch`     | report 1 |
| a commit id that does not exist (`11111111…`)     | rejected, `commit_missing`    | report 2 |
| the worker's own base commit (`ebbf59ea…`)        | rejected, `not_new_on_branch` | report 3 |

Each rejection is recorded with the claimed identity and the evidence the daemon checked, and none produced a message to the PM. The CLI answered with exit code 4 and the reason. Verdict: **PASS**.

## Case (b): the known hole

A second process made a commit in the worker's worktree (`2534ac36…`, author `forger <forger@example.com>`, on branch `capstan/developer-1-g1`) and reported it with the same stolen token. **The report was accepted** (report 4) and the PM notice was queued, with the sender recorded as the controller's internal actor.

This is recorded as the known hole of DEC-005, **not as a pass**. The ledger row shows the claimed identity (`developer-1`, generation 1, the actor `d967861e…` behind the token) and the checked evidence (the commit exists, lies after the base commit and is an ancestor of the branch tip). It does not and cannot show who made the commit: authorship is not part of the check, and any process of the same user can commit into any worktree and use any agent's token.

## What the controller checks, and what it does not

Checked by the daemon from the ledger and git, never taken from the agent: the agent is active; its current generation; the branch and base commit recorded for that generation; that the commit exists, is an ancestor of the branch tip and is neither the base commit nor an ancestor of it. Every git call uses an argument array, a clean environment (no inherited `GIT_*` variables, no user or system git configuration) and `--no-replace-objects`; tests cover a `GIT_DIR` pointing at another repository and a replace ref. These close the daemon's own environment and replace refs only. A worker shares the repository, so it can still alter the repository's own configuration, object alternates and branch refs; that is the same-user hole of DEC-005, not closed.

Not checked: authorship, that the work is good, that the worker made the commit, and whether a commit on the branch was made there or merged in from `main` (a commit the worker merges into its branch counts as on its branch). A report is a verified fact about the repository, not a review; acceptance needs the Stage 4 reviewer receipt.

## Not shown by this run

- The worker's use of `cstan report` through its prompt with real Claude (the stand-in does not run commands).
- Delivery of the PM notice to a live PM (it was queued; delivery to the PM is the Stage 2 pull path and is covered by its gate).
- A second generation of a worker (replacement is Stage 6); only the naming and the check against the recorded branch are in place.
- The relay for reports accepted while no PM is active runs in the daemon and is covered by unit tests, not by this run.

## State after the run

The worker was released (pane closed, worktree removed, branch kept because it holds a commit), the daemon and the `capstan-s3` Herdr server were stopped, and the operator's default Herdr session showed nothing added or removed compared with its snapshot before the run.
