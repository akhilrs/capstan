# Project workspace and PM layout gate (PM-43)

Run on 2026-10-02 in isolated Herdr sessions (`capstan-s10`, `capstan-s11`) with the fake Claude stand-in for every agent, so no model usage was spent. The terminal was 240 columns by 60 rows. The default Herdr session has the same workspaces before and after.

## Herdr facts checked first (0.9.3)

- Closing the only pane of a workspace that has worktree children is refused (`confirmation_required`, "would close a worktree group"). Closing it is allowed when the workspace has another pane, or another tab.
- `pane split --ratio` and `pane move --ratio` give the original pane that fraction (a 174-column pane split at `0.4` left 70 columns).
- `herdr tab create --workspace ID --label TEXT` adds a tab with its own root pane; `tab rename` names it.

## What ran

1. **Pane mode (`alpha`).** `cstan start` created one workspace, `alpha`, with tabs `pm` and `watch`. After three `cstan spawn developer` calls the PM pane was 128 columns wide of 214 (60%) and 59 rows high (full height), at the left; the three workers were stacked in the right column (rows 15, 15 and 29, all 86 columns wide). The PM pane was never split down.
2. **`cstan pm restart` (`alpha`).** The old PM pane was closed (allowed, the `watch` tab keeps the workspace) and the new PM started in a new tab `pm`; workers stayed where they were.
3. **Tab mode (`beta`).** The Spaces tree showed one `beta` workspace with the two worker worktrees (`beta · developer-1`, `beta · developer-2`) as its children, and no separate `pm` or `watch` workspace.
4. **Upgrade (`legacy2`, started with the previous build).** The previous build had made `legacy2 · pm` and `legacy2 · watch` and run a worker as a pane. The new build's `cstan start` adopted both agents and renamed them (`legacy2-pm-1`, `legacy2-developer-1`), relabelled the old watch workspace `legacy2` (it is now the project workspace), and logged the ignored `layout.split` key. `cstan pm restart` then started the PM in a `pm` tab of that workspace (two tabs, two panes) and closed the old PM's own workspace pane.

## Defects the run found and this change fixed

- A hub made while a PM is already running (here because rebuilding `dist` killed the old watch process) left an empty shell as its first tab. The empty root pane is now closed when no PM takes it.
- The old watch workspace kept its `· watch` name because adoption ran before the hub check; adoption now relabels it.
- A gone watch pane lost its workspace id when adoption cleared the row, so a new workspace was made; the row is kept now and the watch tab is made again in the same workspace.

## Limits

- After a restart the new PM is alone in its `pm` tab; workers placed beside the old PM stay in the old tab until released. Herdr orders the tabs `watch`, then the restarted PM's `pm`.
- Stacking splits the tallest worker pane in half, so heights are unequal (15, 15, 29).
- The first PM start needs the project workspace: if it cannot be opened, the PM does not start (before, the PM started without the watch view).
- Fake agents only. The real-Herdr tests in `npm run check` (the launcher and adapter live tests) are timing-sensitive when run next to other heavy processes; they passed alone and in the final full run.
