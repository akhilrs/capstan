# Workspace views: per-repo work, integration sets and worktrees (plan-185, ws-design)

Status: design for the user's approval. ws-feed, ws-dash, ws-apps-mac and ws-apps-ios wait for it.

Claude Design: https://claude.ai/artifact/Jj4xc5W5NftajBnMSrJSkg (canvas "Capstan — workspace views (plan-185)", boards 00 to 08; the app boards each in light and dark). The canvas is private until it is shared from its Share menu.

Direction: **A, Ledger strip**, picked by the user on 2026-11-11 (board 00 shows A next to B, Stacked members).

Strings: `docs/design/plan-185-ws-design-strings.json` holds every new user-visible text with its key and the surfaces that use it. The keys named under each frame below are the ones that frame shows. Texts under `shipped.` are quoted as the controller sends them today and must not be reworded.

Screenshots (static renders of the boards, `source/render.html`):

| File | Board | Shows |
|---|---|---|
| `docs/design/plan-185-ws-design/00-directions.png` | 00 | A and B side by side, one 4-repo set |
| `docs/design/plan-185-ws-design/01-vocabulary.png` | 01 | the shared state words and glyphs; truncation and overflow rules |
| `docs/design/plan-185-ws-design/02-cstan-text.png` | 02 | `cstan status` set lines (all states), multi-repo report, per-repo verify, per-repo cleanup, `plan show --brief` repos, agent repos, NO_COLOR, ASCII, shipped refusals, single-repo reference |
| `docs/design/plan-185-ws-design/03-dash.png` | 03 | dash pipeline panel (all states, selected set), NO_COLOR, agents panel with worktrees, 80 columns with 32 repos, single-repo reference |
| `docs/design/plan-185-ws-design/04-mac-inbox.png` | 04 | Mac Waiting for you rows and the set inspector, light and dark |
| `docs/design/plan-185-ws-design/05-mac-inspector-states.png` | 05 | Mac inspector for each set state, a multi-repo report, loading, older host |
| `docs/design/plan-185-ws-design/06-mac-merge-sheet.png` | 06 | Mac merge sheet: next repo, order refusal, merged, merged not confirmed, last repo, review open |
| `docs/design/plan-185-ws-design/07-mac-workspace.png` | 07 | Mac Workspace overview: repos, worktrees and change folder, cleanups by repo; loading, empty, older host |
| `docs/design/plan-185-ws-design/08-iphone.png` | 08 | iPhone inbox rows, merge sheet (next, refusal), merged not confirmed, widget line, single-repo reference |
| `docs/design/plan-185-ws-design/focus-tab-check.png` | 06 | Tab traversal with the focus ring |

Sources: `docs/design/plan-185-ws-design/source/project/` (the canvas files), `source/render.html` (renders one board without the canvas runtime), `source/contrast.js`, `source/gen/dash.py` (writes the dash frames at the exact panel width).

## Brief

- **Users:** the Capstan operator, in the terminal (`cstan`, `cstan dash`) and on the Mac and iPhone apps. Several visits a day while a workspace plan runs. The fast task: "which repo of this set merges next, and is anything blocking it".
- **Purpose:** show the workspace data plans 181, 183 and 184 record: commits per repo in a report, verify per repo with one gate, an integration set with members in merge order (dropped, redone, confirmed), decide-merge one member at a time, per-repo cleanups and agent worktrees.
- **Tone:** exact, calm, operational. Axes (1 to 5): task intensity 4 (operate), data density 5, brand expression 1, audience 1 (expert).
- **References (traits only):** GitHub's merge queue (entries in order, the head of the queue named, a refused entry says what it waits for); Buildkite's pipeline view (one row per build, a strip of step states with a shape per state); `git log --graph --oneline` (dense, glyph plus short sha, one line per thing); Xcode's Issues navigator (a group with a count, opening to its items); Apple Wallet pass stacks on iPhone (a group shown as one card with a summary, details on open).
- **Constraints:** four surfaces, one vocabulary (the plan's risk). Terminal: 80 and 120 columns, `NO_COLOR`, `TERM=dumb` ASCII, the dash theme roles (`dash/src/view/theme.rs`). Apps: macOS 26 and iOS 26 SwiftUI, SF Pro and SF Mono, light and dark follow the system, Dynamic Type, VoiceOver, the approved Glass Workbench and Glass Inbox looks. WCAG 2.2 AA. Host text (branch names, repo names) is plain text, never a link or rich text. A set holds at most 32 repos; repo names up to 32 characters. No new action: deciding stays the shipped decide-merge per member. Estimate: 6 hours.
- **Inputs read:** no DESIGN.md, no `docs/design/rules.md` or `directions.md`; `cstan design guide rules`; the Capstan design system README and tokens (installed on the canvas); `docs/design/mac-app.md`, `iphone-app.md`, `plan-137-merge-inspector.md` (the "never a bare merged" rule), `plan-169-model-design.md` (board CSS reused); the dash showcase fixture and `glyphs.rs`, `theme.rs`, `panels/pipeline.rs`; `cstan` `compact.rs`, `usage.rs`; migration `0063_workspace_repos.sql`; kernel `workspace.rs` (set states and moves), `remote_decisions.rs` (`merge_order_block`), `integrations.rs`, `snapshot_feed.rs`, `cleanup.rs`; daemon `decide_merge.rs`, `verify.rs`, `reports.rs`.
- **Content extremes:** 32 repos in a set; 32-character repo names (`payments-reconciliation-service` is 31, `notifications-fanout-worker` 27); a member with no head yet; a member redone once (attempt 1, shown "attempt 2"); a dropped member; a repo that left the configuration with leftover worktrees; a report naming 3 repos on one branch.
- **ui-ux-pro-max pass:** its generated palette and fonts were not used (the Capstan system and the approved app looks win). Kept: truncation with ellipsis plus a way to the whole text, colour never alone, 44 pt targets on the phone.

## One vocabulary for four surfaces

Board 01. Every surface uses these words and glyphs; colour is added on top.

| Feed value | Word | App long label | Glyph | ASCII | SF Symbol | Dash role | App colour | Key |
|---|---|---|---|---|---|---|---|---|
| `running` | building | Building | ◌ | `.` | circle.dotted | dim | secondary | `state.building` |
| `merged` | built | Built, not in main | ○ | `o` | circle | info | blue | `state.built` |
| `confirmed` | in main | In main | ● | `#` | circle.fill | ok | green | `state.inMain` |
| `conflicted` | conflicted | Conflicted | ▲ | `!` | exclamationmark.triangle.fill | warn | yellow | `state.conflicted` |
| `failed` | failed | Failed | ✕ | `x` | xmark.circle | bad | red | `state.failed` |
| `discarded` | discarded | Discarded | – | `-` | minus.circle | dim | secondary | `state.discarded` |
| `dropped: true` | dropped | Dropped | – | `-` | minus.circle | dim | secondary, name struck through | `state.dropped` |

- The feed's `merged` means merged into the integration branch, so it reads **built** on every surface (the approved plan-137 rule: no bare "merged"). The shipped feed title of a member (`shipped.memberTitle`, "merged <branch> (repo r of set s)") is unchanged for older clients.
- A set's word comes from its members (`set.summary.*`): building · 2 of 4 built / built · none in main yet / 1 of 3 in main / in main · all 4 repos / conflicted in web / failed in worker / discarded · nothing reached main. The dash uses the short forms `set.short.*` (2/4 built, 1/3 in main, conflicted: web).
- Markers that are not states: **next** (the first live member not in main; the host refuses any other with "merge <x> first"), **review subject** (an underline under its glyph in a strip, a tag in rows), **attempt n** (n = attempt + 1, shown only after a redo).
- "Live" members exclude dropped ones: the counts, the gate, the review and confirm ignore a dropped member; it is still listed, last word "dropped", note "Outside the gate, review and confirm".

## Truncation and overflow

Board 01, and drawn on 02, 03, 04 and 08.

- **Repo names (up to 32 characters):** whole wherever a row wraps or has room (cstan member text sizes its column to the longest name; the Mac member rows fit 32 characters of SF Mono 12 in the 360 pt inspector). Cut in the middle where the column is fixed: the dash REPO column (13 cells, `payme…service`, with a hint line `payme…service = payments-reconciliation-service` for the selected member) and iPhone member rows (row width; the detail wraps the name whole). Middle cut keeps the start and the suffix (`-service`, `-worker`) readable.
- **32 repos:** the strip shows a fixed number of glyphs, then `+n`; the count sentence always covers every repo. cstan and the Mac inbox: 12 glyphs then `+20`. Dash STATE column (10 cells): up to 10 glyphs, else 7 and `+25`. iPhone inbox: 8 glyphs then `+24` (VoiceOver: "and 24 more repos"). The glyphs shown are the first in merge order; when the next repo is beyond them, the strip starts two before it with a leading `+n`.
- **Member lists** (Mac inspector and sheets, iPhone sheet, dash selected area) scroll; the next repo is scrolled into view when the list opens. The dash shows `▲ n above` / `▼ n below`.
- **Branches** longer than 32 characters are cut in the middle in a set line only; inspectors wrap them.

## cstan text (board 02)

- **Status, sets:** one line per set: `set <id> <branch>  <strip>  <summary>` (`set.line`, `set.summary.*`, `set.next`). A second dim line only for members that need a word: `review subject: worker · dropped: infra` (`set.reviewSubject`, `set.dropped.list`), `web conflicted: cstan integrate redo 3b81e2c4 --repo web` (`member.conflicted.redo`), `worker failed, attempt 2` (`member.failed`). Drawn for all eight states (all building, partly built, one conflicted, one failed after redo, one dropped, discarded, all built, all in main), the decide-order case (1 of 3 in main · next: web) and 32 repos.
- **Status, reports:** `<id> <agent> accepted 3 repos  api 9f3c2a1 · web 51ab07e · worker c04d9e2  on <branch>` (`report.repos`, `report.commit`, `report.branch`).
- **Verify of a set:** `set <id> verify  gate waiting: worker` then the per-repo line `api passed · web passed · worker running · infra outside the gate` (`verify.gate.*`, `verify.repo`, `verify.repo.outside`, `verify.repo.lastPassed`). The shipped `cstan verify` lines ("verify run run-1 for repo web queued; …") are unchanged.
- **Cleanup:** one offer per repo, `cleanup <id> · <repo>: branches n, worktrees m` (`cleanup.repo`). A repo that left the configuration: `cleanup.repo.left` in warn, its worktrees listed by branch and path, the shipped reason (`shipped.cleanup.left`) and the `cleanup apply <id> <branch>…` line, since those entries go only by name.
- **plan show --brief:** each package line gains `repos capstan,capstan-dash-fixtures` after its progress (`plan.repos`, comma-joined like `deps`).
- **Agents:** the agent line gains `· 3 repos` (`worktree.count`).
- **NO_COLOR / TERM=dumb:** same text; glyph shapes or ASCII letters carry the state with the word.
- **Shipped refusals** quoted unchanged: `shipped.redo.confirmed`, `shipped.redo.running`, `shipped.review.open`, `shipped.verify.confirmed`.

## cstan dash (board 03)

- **Pipeline panel:** a set is one row, STAGE `set`, STATE the strip, WHO / COMMIT `<set id> <short summary>`, AGE. The members' own integration items fold into it. The stage line counts a set once with set words (`integrations … 9  built 4  in main 1 …`). A multi-repo report row shows `developer-4 3 repos 9f3c2a1` (first commit).
- **Selected area** (set selected): `set <id> <branch>  <summary>`, then `# REPO STATE HEAD VERIFY` in merge order; the next member carries `▌` and `next`, the review subject `review`, a dropped member is dim with `outside the gate`; the gate line last (`verify.gate.*`).
- **Agents panel:** a REPOS column (count; `–` for none) at 100 columns and wider, left out first when narrow. The selected area lists the agent's worktrees (`worktree.agentHeading`, repo and path) and its change folder.
- **80 columns, 32 repos:** drawn; the selected area scrolls around the next repo.
- **Keys:** unchanged; Enter opens the selected area, ↑↓ moves through members, Esc returns. No new action key.
- **States:** empty uses the existing "no integrations" line; loading and link errors are the existing link states; focus is the heavy focused border and `▌`, as today. Under NO_COLOR the selected row is underlined (shown).

## Mac app (boards 04 to 07)

- **Inbox rows (04):** "Integration sets" group. A set row: leading glyph of the set word, title `Set <id> · <summary title>` (`set.title.app`), age, second line the strip then the reason it waits (`set.next.app` "Merge web next", `member.conflicted.app`, `member.failed`). Selected, hover and focus drawn (accent selection, `fill2` hover, 3 px focus ring). A multi-repo report row: `developer-4 · 3 repos`, then `api 9f3c2a1 · web 51ab07e · worker c04d9e2 on <branch>`.
- **Integration detail rows (04, 05):** the inspector lists "Repos in merge order · 3 repos · 1 dropped" (`set.members.heading`, `set.members.count`): position, glyph, repo (mono), word, then head sha and the member's verify (`member.sub`). The next member has the accent tint and a **Next** tag; the review subject a **Review subject** tag and the underline; a dropped member's name is struck through, word "dropped", note "Outside the gate, review and confirm". Then the gate banner (`verify.gate.*` with `verify.gate.leadWaiting`) and actions (Copy commands, Merge <next>…). Board 05 draws all building, partly built, one conflicted (redo command, Message PM…), one failed after redo (attempt 2, last passed sha, discard command), one dropped, discarded, all in main, a report in three repos (Commits with verify per repo), loading (skeleton) and an older host (members stay separate integrations with their shipped titles).
- **Merge sheet (06):** decides one member at a time (`decide.title` "Merge web into main", `decide.order` "Repo 2 of 3 in merge order", the member list, the gate, `decide.after` "After this: worker"). Out of order, the host's refusal is quoted as sent: banner **merge web first** (`shipped.mergeOrder`) under the title `decide.blocked.title` "worker has to wait", with `decide.blocked.lead` and **Open web**. After a merge: "web is in main · 7d1c0e9" (`decide.done`) and **Merge worker…**. Confirm per repo: when the merge is made but recording it failed, `confirm.title` and `confirm.lead` with `cstan integrate confirm 8a46cad2 --repo web` (`confirm.command`) and Copy command. The last repo says "This is the last repo of the set." A review open is the shipped "a review of set 8a46cad2 is open".
- **Workspace overview (07):** a new sidebar item "Workspace": Repos in merge order (position, name, main branch, worktrees, open sets), the selected agent's worktrees with branch, path and the change folder, and Cleanups by repo with the left-the-configuration banner, its shipped reason and the by-name apply command. States: loading, no cleanups, older host.

## iPhone app (board 08)

- **Inbox rows:** one row per set, title wraps to two lines, second line the strip (8 glyphs then `+n`) and the reason it waits.
- **Merge sheet:** large sheet; eyebrow keeps the shipped "repo web of set 8a46cad2"; title, order, members in merge order with Next, Review subject and dropped; gate and After this; Cancel / Merge web (50 pt buttons). The order refusal is a medium sheet with **merge web first** quoted and Open web.
- **Merged, not confirmed:** the confirm command with Copy command.
- **Widget line:** the medium widget's kind line reads "Integration sets 8" and counts a set once (`widget.sets`); no member or repo name reaches the widget.

## Single-repo projects

Nothing changes for a project without `[repos]`: no new key is used, no line, row, column or member is added, and the JSON stays byte-identical (ws-feed's capture test). One unchanged frame per surface is drawn for reference: cstan (board 02, bottom right), dash (board 03, the showcase pipeline excerpt), Mac and iPhone (board 08, bottom: the plan-137 row "integration/plan-12 · Built · not in main"). The Mac inbox and inspector for a single-repo integration are the approved plan-137 boards.

## Accessibility

- **No state by colour alone:** every state has a word; in strips each state has its own glyph shape (◌ ○ ● ▲ ✕ –, ASCII . o # ! x -) and the row's sentence states the counts. Dropped adds a strike-through and a note, Next and Review subject are text tags. NO_COLOR and ASCII frames are drawn.
- **VoiceOver (Mac and iPhone):** a set row reads `a11y.set.row`, e.g. "Set 8a46cad2, 1 of 3 in main, next web"; the strip is one element (`a11y.strip`): the summary then "api in main, web built, not in main, worker built, not in main, infra dropped, outside the gate, review and confirm", the first 12 members then "and n more repos". A member row reads `member.voiceover`: "2 of 4, web, built, not in main, next to merge" (review subject and attempt appended when they apply). Glyphs are hidden from VoiceOver; the merge sheet's order refusal is announced as an alert with its quoted text.
- **Keyboard (Mac):** Tab order on the merge sheet board: Cancel → Merge web → Close → Open web → Done → Merge worker… → Close → Copy command, each with a 3 px solid ring (checked by Tab traversal in the render). The inbox list stays one Tab stop with arrow keys, as approved.
- **Dynamic Type (iPhone):** set titles wrap to two lines; the strip keeps its glyph count and the reason line wraps.
- **Contrast:** computed, below.

## Verification

- **Screenshots:** Playwright rendered every board with `source/render.html` at 1900 px wide (files above). The canvas page itself was not screenshotted. Light and dark are the boards' own `.light` and `.dark` token sets (apps) and the dark terminal plus NO_COLOR (terminal; the dash has no light theme). The 375 and 768 px page widths do not apply: the frames are a Mac window, a 402 pt phone and fixed terminal widths (80 and 120 columns are drawn).
- **Contrast (computed in the page, each colour composited over its real background):** 2,079 text elements on the nine boards, all at or above AA after two fixes (the inspector panel style was missing from the copied CSS, and the dash's selected row now draws its words bright on `selectBg`, as the real dash does). Lowest per board: 00 5.87, 01 5.32, 02 5.32, 03 5.32, 04 4.55 (the dark "Next" tag on the accent tint), 05 4.64, 06 4.64, 07 4.91, 08 4.98.
- **Keyboard:** Tab traversal on board 06 (above).
- **Not verified:** SF Pro, SF Mono and IBM Plex metrics (the render used fallback fonts, so the ✕ glyph is slightly wide in some frames and real truncation points differ); SF Symbols (unicode stand-ins drawn); VoiceOver and Dynamic Type on devices; Liquid Glass; a real terminal's ambiguous-width handling of ◌ ○ ● ▲ (the dash already uses ● ○ ▲); the native and ratatui builds.

## Self-critique

Avoid list: default gradients **pass** (none). Glassmorphism everywhere **pass** (glass only on the existing sidebar, toolbar and tab bar). Emoji or icon grids **pass** (state glyphs carry meaning, no decorative icons). Three identical cards in a row **pass**: the board 05 and 06 galleries are different cases side by side for comparison, not a layout. Centred hero **pass** (none). Everything rounded and soft-shadowed **partial**, inherited from the macOS and iOS shapes the user chose. Lorem ipsum **pass** (realistic ids, repos and shipped texts). Generic Inter on white **pass**: Inter only stands in for SF Pro in the app mockups; boards use IBM Plex per the design system.

Require list: two typefaces **pass** per surface (SF Pro and SF Mono; IBM Plex Sans and Mono on the boards; one mono in the terminal). Spacing scale **pass** (4 px; every value in `ws.css` is on it). One accent **pass** (amber only for the next member, the primary action and selection). Hierarchy and asymmetry **pass** (one line per set, detail on open; the next repo is the one tinted row). Real content **pass**. Motion with meaning **pass** (none added; sheets use the system presentation). Empty, loading, error, hover and focus **pass** (05, 07 and 03 notes; hover and focus on 04 and 06).

Rules: T1 pass, T2 pass, T3 deviation (below), T5 pass, T6 pass (capitals only in the terminal's existing headers), T7 pass (tabular figures for counts and positions), C1 pass, C3 pass (computed), C4 pass (each state colour has a glyph and word), C5 pass, C6 pass (dark drawn per theme), L1 pass, L6 pass, L8 pass (80 and 120 columns, narrow phone), L9 pass, P1 pass, P3 pass (Merge web, Open web, Copy command), P4 pass (counts are exact: 1 of 3, 5 of 32), P5 pass, P6 pass (sentence case), K3 pass (one primary per sheet), K5 pass (row heights unchanged; overflow rules stated), K6 pass, K7 pass (one vocabulary), A1 pass, A3 pass, A4 pass, A6 pass.

## Rule deviations

- **T3:** 12 px row captions on the Mac are the macOS type styles; inherited from `mac-app.md`.
- **K1:** concentric radii on the apps, not the design system's 4 px; the user's decision of 2026-10-28, inherited.
- **Design system:** the apps keep glass and rounded shapes over the web system's "no glass, 4 px" (same decision). The terminal frames use the dash theme roles, not the system's colours, because the dash's colours are shipped.
- **C1 (partial):** the dash selected row draws state words bright instead of in their colour, so contrast holds on `selectBg`; the glyph and word carry the state.

## Design record changes

No DESIGN.md exists and this package creates none. Proposed for when one is written:

- **A shared state vocabulary** (the table above) as the record's state words, with glyph, ASCII, SF Symbol and colour role per state, and the rule that a feed value is never shown raw when it misleads (`merged` reads "built").
- **The strip pattern:** one glyph per member in a fixed order, a cap per surface and `+n`, always with a count sentence; the review subject underlined; the next member named in words.
- **Overflow rules:** middle truncation for identifiers in fixed columns, the whole value one step away (hint line, inspector, tooltip).
- **Group rows:** a group of host items (a set) is one row; its members open in place; old clients keep seeing the members as items.

## Kept, moved, changed, removed

- **Kept:** every single-repo row, line, column and frame; the shipped titles and refusals; the approved plan-137 pills and inspector order; the dash keys.
- **Moved:** in a workspace project, the members' integration rows move under their set row (dash pipeline, Mac and iPhone inbox); the members stay in the feed for old clients.
- **Changed:** the dash pipeline stage line counts sets; the Mac and iPhone merge sheets name the repo and its place in the order; the dash agents panel gains REPOS at 100+ columns.
- **Removed:** nothing.
- **Added:** the Mac Workspace sidebar item.

## Open questions

1. **"built" for the feed's `merged`.** The vocabulary follows plan-137 on all four surfaces, including cstan and the dash in a workspace project, while single-repo dash rows keep showing `merged` as today. Accept the difference, or should single-repo terminal rows also say "built" (a change outside this plan)?
2. **Mac Workspace sidebar item.** The plan asks for a "workspace overview"; drawn as a new sidebar item under Plans. ws-apps-mac owns `Workspaces/**`; the sidebar entry may touch a file outside its owns. Confirm with the architect.
3. **Confirm in the apps.** The apps confirm through decide-merge (the merge confirms); when recording fails they show the CLI command instead of a Confirm button, as plan-137 does. Should the apps get their own Confirm action later?
4. **Strip cap of 12 on cstan.** cstan could show all 32 glyphs on a second line; drawn with the same cap as the Mac for one rule everywhere.
