# plan-181 ws-repo-add: manual check of `cstan repo add` with a real daemon and Herdr

Run on 2026-11-11 against a scratch setup only: scratch `HOME` and XDG dirs (`env -i`), a throwaway Herdr session
`capstan-test-repoadd`, a scratch project (a git repo) and three sibling repos `core`, `web`, `extra` (each one commit on
`main`) under `/tmp/re2e`, the branch's own debug `cstan` and `cstan-daemon`, `CAPSTAN_WORKSPACE_PREVIEW=1`. This
project's controller, state and panes were not touched. `capstan.toml`: `[architect] enabled = false`, the roles pm and
developer on a `claude` host, `[repos.core|web|extra]` (`../repos/<name>`, `main_branch = "main"`),
`[workspace] default_repo = "core"`, `worktree_dir = "../changes"`.

## Steps and output

1. `cstan daemon` (background), then `cstan ping`: ok. `cstan start`: `launch: state: started`, `agentId: pm-1`.
2. A plan cannot be opened without an architect, so plan `plan-ws` (approved, one package `pkg` with
   `repos = ["core","web"]`) was inserted into the scratch ledger with `sqlite3`, as the daemon test seeds it.
3. `cstan spawn developer --task plan-ws/pkg`: `state: started`, `agentId: developer-1`, `worktreePath:
   /tmp/re2e/changes/developer-1`, `branch: feat/plan-ws-pkg-pkg`. The folder held `core` and `web`.
4. The worker's own `claude` is not signed in under the scratch home, so the command ran with the environment of the
   worker's pane process (its `CAPSTAN_TOKEN` and socket, working directory the change folder), through a script:
   - `cstan repo add extra` (package worker): `cstan: rejected: not_in_package: repo extra is not in package plan-ws/pkg`,
     exit 4; no `extra` folder, ledger rows still `core`, `web`.
   - `cstan repo add ghost`: `cstan: rejected: unknown_repo: repo ghost is not in [repos]`, exit 4.
5. `cstan release developer-1`: `state: released`. Then `cstan spawn developer` (ad-hoc): `agentId: developer-2`, branch
   `chore/developer-2-developer`, folder with `core`.
6. From developer-2: `cstan repo add extra` printed `agentId: developer-2`, `repo: extra`, `worktreePath:
   /tmp/re2e/changes/developer-2/extra`, `branch: chore/developer-2-developer`, `baseSha: 2ae1c9a4...`, exit 0.
   `git status -sb` there: `## chore/developer-2-developer`; its HEAD equals `main` of `repos/extra`. The ledger had
   rows `core` and `extra`, none removed.
7. Again: `cstan: rejected: already_added: developer-2 already has a worktree in repo extra`, exit 4.
8. `cstan release developer-2`: `worktreeRemoved: true`, `branchKept: false`. `changes/` empty, `git worktree list` in
   `extra` shows only the repo, branch `extra` has only `main`, both ledger rows removed.
9. `cstan stop`; the Herdr session stopped.

## Cleanup

Nothing was added to the code for this run (no temporary harness or line). One side effect: `cstan trust` writes to the
user's trust store under the passwd home, not the scratch `HOME`; the one file it created for the scratch project was
deleted afterwards. The scratch directory `/tmp/re2e` holds only the setup above.
