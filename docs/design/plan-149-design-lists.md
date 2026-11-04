# Agents and Plans for many projects, Mac (plan-149, design-lists)

Status: design for the user's approval. mac-lists waits for it.

Claude Design: https://claude.ai/artifact/Bx2tk4JC2fVsyPzu1fT4fg (canvas "Capstan for Mac — Agents and Plans for many projects (plan-149)", artboards 20 to 26, each in light and dark).

It extends the approved Glass Workbench design (direction A, https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8, `docs/design/mac-app.md`). It uses the same `base.css` and tokens, plus `lists.css` for the new parts, and the Capstan design system is installed as the record. The approved canvas is unchanged. It brings the iPhone redesigns of Agents (#67) and Plans (#68) to the Mac split view (sidebar, list, inspector).

Screenshots (static renders of the artboards, light above dark):

- `docs/design/plan-149-design-lists/20-agents-all-projects.png`
- `docs/design/plan-149-design-lists/21-agents-one-project.png`
- `docs/design/plan-149-design-lists/22-agents-states.png`
- `docs/design/plan-149-design-lists/23-plans-all-projects.png`
- `docs/design/plan-149-design-lists/24-plans-one-project.png`
- `docs/design/plan-149-design-lists/25-plans-states.png`
- `docs/design/plan-149-design-lists/26-compact-window.png`

## Brief

- **Users:** the Capstan operator on a Mac who runs many projects (the boards use 22 projects, 112 agents and 122 plans on 3 hosts). They visit several times a day. The fast task: "which agent or plan needs me, in any project", then "how is project X doing".
- **Purpose:** the Agents and Plans screens stop being one long list when there are 20 or more projects. Attention comes first, every project folds to one row, and one project can be chosen.
- **Tone:** calm, exact, native. Axes (1 to 5): task intensity 4 (operate), data density 4, brand expression 2, audience 2 (expert).
- **References (traits only):** Xcode navigators (groups that fold, with counts, and → ← to open and close them); Apple Mail on macOS 26 (a toolbar pop-up for the scope, a search field in the toolbar, an inspector that follows the selection); Activity Monitor (a segmented filter whose labels carry live counts); Linear's project list (one dense row per project with a progress bar and a count in words); macOS Finder sidebar (Pin and Unpin from a context menu, pinned items first).
- **Constraints:** macOS 26 SwiftUI (NavigationSplitView, `.inspector`, `.searchable`, `Menu`/`Picker` in the toolbar, `DisclosureGroup`, `contextMenu`); window from 640 × 480 to wide; SF Pro and SF Mono; light and dark follow the system; WCAG 2.2 AA; untrusted text never rich text. The groups, counts and orders are the iPhone ones, moved to CapstanAppCore by kit-shared (`AgentGroups`, `PlanGroups`, `ProjectPins`). No data the host does not send: no finish date, no time since a stall. Estimate: 4 hours.
- **Inputs read:** no DESIGN.md, no `docs/design/rules.md` or `directions.md`; the Capstan system README (web look; the Mac app keeps the accent, voice and mono rule, and follows macOS for materials and shapes, as the user decided on 2026-10-28); the approved canvas files (`base.css`, artboards 06, 09 and 10); `apps/ios/CapstanPhone/Agents/AgentsScreen.swift` and `Plans/PlansScreen.swift` (#67, #68); the Mac `AgentsScreen`, `PlansScreen` and `SidebarView`; `AgentRow` and `PlanRow` for the fields that exist.
- **Content extremes:** project names up to 64 bytes (one line, ellipsis); 22 projects and more; a project with 8 agents (four, then "Show 4 more"); a project with 22 Done plans (ten, then "Show N more"); two projects with the same name on two hosts.
- **Directions:** none offered. The package fixes direction A, Glass Workbench, which the user approved for the whole app.

## Screens and states

| # | Board | Shows | States |
|---|---|---|---|
| 20 | Agents · all projects, wide window | summary "112 agents in 22 projects on 3 hosts"; **Needs attention · 5** card across projects (needs you, then stalled, lost, blocked); filter All 112 · Attention 5 · Working 38 · Paused 6; "Pinned · 2" then "Other projects · 20", A to Z; one row per project: name, pin glyph, host, agent count, count bar, counts in words; an open project shows four agents then "Show 4 more"; the inspector shows the selected agent | selected attention row, open and closed groups, hover, many projects (list scrolls under the toolbar) |
| 21 | Agents · one project | project menu set to payments-api; the approved per-project table (09) and the project pause bar; the filter counts become the project's (8 · 4 · 2 · 1); the inspector with Pause and Resume as on 09 | selected row, disabled Resume |
| 22 | Agents · states | Attention filter (card hides, matching projects open, every row shown); searching "reviewer" (ids and roles; a project or host match keeps all its agents; "1 of 8" per group); empty: no agents, nothing for this filter, no search results, each with its action; loading the first snapshot; group row default, hover, keyboard focus, selected, host offline; the context menu (Pin, Show only, Open all, Close all); the project inspector; the keyboard map | empty, loading, error (host offline), hover, focus |
| 23 | Plans · all projects, wide window | summary "122 plans in 22 projects"; **Needs you · 3** card; Active 31 · In review 4 · Done 87; "Active · by project", one folded row per project (plan count, package bar, "4 of 12 packages integrated"); a project the user opened shows its compact plan rows; the legend; the approved plan inspector (10) | selected plan, hover, open and closed groups |
| 24 | Plans · one project | project menu set to payments-api; Needs you · 1; "Active · by state": Waits for you (open, accent), In progress (open), Drafts (folded); newest plan first | selected plan, folded drafts |
| 25 | Plans · states | the project menu with 22 projects (All, Pinned, A to Z, counts, type to jump, scrolls); In review grouped by project; Done for one project, ten then "Show 18 more"; searching "ledger" (titles and ids; the filter counts follow the search); empty: no plans, nothing in this state, no results; loading and a host offline | empty, loading, error, highlighted menu item |
| 26 | Compact window 640 × 480 | Agents and Plans at the minimum size: sidebar folded, filter as a pop-up ("All 112"), search as a button, the attention card shows three then "Show 2 more", the inspector as a sheet | keyboard focus, sheet |

Hover is the 150 ms `fill2` step, focus the 3 px `focus` ring, selection the accent tint with an accent outline, all as on 06. Groups open and close with the system disclosure animation (250 ms); under Reduce Motion only the chevron turns. Nothing else moves.

## Behaviour

**Orders and counts (the iPhone ones, from CapstanAppCore).**
- Agents inside a project: needs you; stalled, lost, blocked; working; paused; idle. The attention card uses the same order across projects.
- Projects: pinned first in pin order, then A to Z by project name, then host name.
- Plans inside a group: newest first. Done has no date grouping because the host sends no finish date.
- Agents shows four agents in an open project, then "Show N more". Plans shows ten plans in a group, then "Show N more". While a filter or a search is on, every match shows.
- With all projects, Agents projects start folded; a filter or a search opens every project that matches, and a project the user folds stays folded until the filter changes. With all projects, Plans groups start folded. With one project, Waits for you and In progress start open and Drafts starts folded.
- The count bar: needs you (accent), stalled, lost or blocked (red), working (green), paused (blue), idle (grey at 35 %). The words beside it carry the same counts, so colour never carries meaning alone. The plan bar: integrated (green), reported (blue), assigned (grey), not started (empty); the legend says so.

**Scope.** One toolbar pop-up, shared by Agents and Plans: All projects; Pinned; all projects A to Z, each with its count. Type to jump, ⌘J opens it. Two projects with the same name show their host ("api · build-host"). Choosing one project shows the per-project view: on Agents the table and the pause bar (21); on Plans Active grouped by state (24). Return on a group row, and "Show only …" in its menu, do the same.

**Search.** In the toolbar (⌘F, Esc clears). Agents: agent ids, roles, project names, host names. Plans: titles and plan ids. The attention and Needs you cards hide while searching; the filter counts follow the search.

**Pins.** Context menu on a project row: "Pin <project>" or "Unpin <project>"; also "Show only <project>", "Open all groups" (⌥→) and "Close all groups" (⌥←). The project inspector has a Pin button. Pins are kept per Mac and shared by Agents and Plans.

**Inspector.** An agent row shows the agent (as 09); a plan row shows the plan (as 10); a project row shows the project: its bar, counts, run state, host, Pin and "Show only <project>". Under 900 px the inspector is a sheet, as on 06.

## Keyboard order

Tab: sidebar → toolbar (project menu, filter, search, Message PM) → list → inspector actions. In the list, ↑ ↓ move through attention rows, project rows and agent or plan rows as one list; → opens and ← closes the focused project; Return on a project shows only that project, on an agent or plan selects it. ⌘1 to ⌘4 pick All, Attention, Working, Paused (on Plans ⌘1 to ⌘3: Active, In review, Done). In SwiftUI the list is one Tab stop. Checked in the mockup by Tab traversal on board 20: 30 stops, every one with a 3 px solid ring. In the HTML the list's "Show 4 more" comes before the toolbar because the toolbar floats over the list in the markup; the native order is the one above.

## What differs from the iPhone, and why

1. **Project chips and the jump sheet become one toolbar pop-up.** Chips do not scale to 22 projects in a Mac toolbar; a pop-up with type to jump does, and it is how Mail and Finder pick a scope. Agents gets the same pop-up, where the iPhone only scrolls to a project: on the Mac, one project opens the approved per-project table with its pause bar.
2. **The filter sits in the toolbar** as a segmented control with counts, not in the list. Under 900 px it becomes a pop-up.
3. **An inspector instead of a pushed screen.** Selecting a row keeps the list in place. Selecting a project row shows a project inspector, which the iPhone has no room for.
4. **Context menu instead of touch and hold** for Pin and Unpin, plus Show only and Open or Close all groups.
5. **Agent rows show the branch** in its own mono column, because the wide list has room; the iPhone shows id and role only.
6. **Keyboard:** disclosure with → and ←, ⌘F, ⌘J and ⌘1 to ⌘4.
7. **Compact window:** the attention card shows three rows then "Show N more", so the projects stay visible in 480 px. The iPhone shows the whole card.
8. **One project on Agents** shows the whole table, without "Show N more", as 09 does.

## Kept, moved, changed, removed

- **Kept:** the per-project Agents table, the pause bar and the agent inspector (09); the plan inspector and package list (10); the sidebar's host and project tree.
- **Moved:** the Agents filter (All, Need attention, Paused) gains Working and its counts, and stays in the toolbar. The plans legend moves under the list.
- **Changed:** Agents and Plans default to all projects. The per-project table orders rows by attention (needs you first) instead of by role, matching the iPhone. The Plans filter Active · Done becomes Active · In review · Done.
- **Removed:** the Plans list as one flat column of every project's plans.
- **Sidebar (small changes only):** a pin glyph after pinned projects; each host row shows its project count ("live · 9"); a host with more projects than fit shows "… N more", which opens the rest. Nothing else in the tree changes.

## Verification

- **Screenshots:** Playwright rendered the seven artboards from static copies of the canvas files at 1600 px, light above dark on each board (paths above). The canvas page itself is private and was not screenshotted. A Mac window has no 375 or 768 px layout; the compact window (640 × 480) is board 26, so the 375 and 768 px checks are **not applicable**.
- **Contrast (computed):** a script blended each text element's colour over its composited background on all seven boards. 1,422 text elements were checked; all pass AA, and the lowest is 4.55:1 (the light "Needs you" pill on the selected attention row). One failure on the first run was fixed: the note under board 26 sat on the wallpaper at 3.71:1 and now uses the label colour. Disabled controls and the dimmed host-offline rows were not measured (WCAG exempts inactive parts). Backgrounds behind glass are modelled without blur.
- **Lowest pairs per board:** 20: 4.55; 21: 4.55; 22: 4.89; 23: 4.82; 24: 4.82; 25: 5.24; 26: 4.55.
- **Keyboard:** Tab traversal on board 20 (above). Arrow keys and disclosure are native behaviour and were not exercised in the HTML.
- **Not verified:** real Liquid Glass refraction; SF Pro metrics (Inter stood in); VoiceOver; Dynamic Type; the native build.

## Self-critique

Avoid list: default gradients **pass** (none). Glassmorphism everywhere **pass** (glass only on the toolbar capsules, sidebar, menus, status capsule and sheet). Emoji or icon grids **pass**. Three identical cards in a row **pass**; the three empty states on 22 and 25 are three different causes with different text and actions. Centred hero **pass** (none). Everything rounded and soft-shadowed **partial**, inherited from the macOS concentric shapes the user chose. Lorem ipsum **pass**. Generic Inter on white **pass**; Inter stands in for SF Pro only in the mockup.

Require list: two typefaces **pass** (SF Pro, SF Mono). Spacing scale **pass** (4 px; every new value in `lists.css` is on it). One accent **pass** (rope amber: selection, the primary button, "Needs you" and "Waits for you" headings, the highlighted menu item). Hierarchy and asymmetry **pass** (attention first, 248 / flexible / 360 columns). Real content **pass** (realistic example data, labelled as such by the canvas). Motion with meaning **pass** (disclosure only). Empty, loading, error, hover and focus states **pass** (22, 25, 26).

Rules: T1 pass, T2 pass, T3 and T4 deviation (below), T5 pass (400, 500, 600), T6 pass (no capitals), T7 pass (tabular counts), C1 pass, C3 pass (computed), C4 pass (word or glyph with every state colour), C5 pass, C6 pass, C7 pass (bars have words beside them), L1 pass, L2 pass, L5 pass, L6 pass, L8 pass (640 to wide), L9 pass, K1 deviation (below), K3 pass, K5 pass (row heights 28 to 48 px, one line with ellipsis, selected and hover shown, "Show N more" as the overflow plan), K6 pass, A1 pass, A3 pass, A4 pass, A5 not applicable (native window), A6 pass.

## Rule deviations

- **T3 and T4:** 13 px body and 12 px captions are the macOS type styles; a larger size comes from Dynamic Type. Inherited from `mac-app.md`.
- **K1:** concentric radii (22, 16, 12, 10, 8, capsule) instead of three fixed radii; the user chose macOS shapes. Inherited.
- **Design system:** glass and rounded shapes over the web system's "no glass, 4 px"; the user's decision of 2026-10-28.

## Design record changes

No DESIGN.md exists and this package does not own one. Proposed additions for when one is written: the **project row** (chevron, name, pin glyph, host and count, count bar with words) and the **count bar** colour order as named components; the **project menu** as the one scope control for list screens; the rule that a list over many projects puts attention first and folds every project to one row.

## Open questions

1. **Sidebar Agents badge.** The boards show the attention count (5) on Agents and the Needs you count (3) on Plans. The approved 09 showed 12 without saying what it counts. Which should it be?
2. **"… N more" in the sidebar.** Should a host fold its projects after a fixed number (the boards use 9), or should hosts be disclosure groups the user folds?
3. **Pins across devices.** Pins are kept per Mac, like the iPhone's. Should the Mac and the iPhone share them later?
4. **Show N more on the Mac.** The boards keep the iPhone's four agents and ten plans. A wide window could show more; keep the same numbers?
5. **Stall duration.** 09's title said "for 14 minutes"; the host sends no stall time, so these boards say "has made no progress". Confirm that 09's wording should change the same way in mac-lists.
