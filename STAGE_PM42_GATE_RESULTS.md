# Project grouping gate (PM-42)

Run on 2026-10-02 in an isolated Herdr session (`capstan-s9`) with the fake Claude stand-in for every agent, so no model usage was spent. Three throwaway projects (`alpha`, `beta-shop`, `legacy`) shared that one session. The old build (the commit before this change) started the `legacy` project first. The default Herdr session has the same workspaces before and after.

## What ran and what it showed

1. **Two new-style projects in one session.** `alpha` and `beta-shop` each started a PM and a worker. Herdr listed `alpha-pm-1`, `alpha-developer-1`, `beta-shop-pm-1` and `beta-shop-developer-1`: no name collision although both projects use the ledger ids `pm-1` and `developer-1`.
2. **Labels.** The sidebar workspaces were `alpha · pm`, `alpha · watch`, `beta-shop · pm`, `beta-shop · watch` (and the `legacy` pair).
3. **Metadata.** `herdr pane get` on a worker shows `tokens: {agent: developer-1, project: alpha, role: developer}`.
4. **Upgrade.** The `legacy` project was started with the old build (names `pm-1` and `developer-1`, workspace `capstan-pm`). Restarting it with the new build renamed the worker to `legacy-developer-1` at once and the PM to `legacy-pm-1` once its startup dialog had been answered, and relabelled the PM workspace `legacy · pm`.
5. **Snippet.** `cstan herdr-config` output passes `herdr config check` (`config: ok`).

## Defects the run found and this change fixed

- **Adopt relabelled a shared workspace.** A worker placed as a tab shares the PM's workspace, so relabelling it with the worker's name renamed the PM's workspace. Only the PM's own workspace is relabelled now (test added).
- **Workspace metadata for a worker failed** (`workspace wA not found`): its own workspace is gone once its pane is placed. A worker's start now reports pane metadata only; the PM's and the watch workspace carry the project token.

## Limits

- **An agent still at a startup dialog cannot be renamed.** Herdr refuses (`agent name cannot change while startup is pending`); adoption logs `adopt_failed` and the agent is renamed at the next adoption after the dialog is answered.
- Two projects whose names slug to the same ten characters still collide in one session; the second start fails with Herdr's name error.
- The labels and names carry the project without any Herdr configuration; the `$project` rows need the snippet pasted into Herdr's own config, and the PM-pane placement and one-tree grouping are not part of this change.
- Fake agents only: nothing was checked against a real Claude, Codex or OMP session for this change.
