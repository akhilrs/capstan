# An agent's model and its source, Mac and iPhone (plan-169, model-design)

Status: design for the user's approval. model-apps waits for it (design.decided).

Claude Design: https://claude.ai/artifact/FXShaFo8gNV7uibuCXReg6 (canvas "Capstan — agent model and source (plan-169)", boards 30 to 34; boards 31 to 34 each in light and dark).

It extends the approved app designs: the Mac Glass Workbench (`docs/design/mac-app.md`, the All Agents view of `docs/design/plan-149-design-lists.md`) and the iPhone Glass Inbox (`docs/design/iphone-app.md`). The boards use the same `mac.css` and `ios.css` as plan-153, plus `model.css` for the new parts; the Capstan design system is installed on the canvas as the record. Nothing else on the approved canvases changes.

Screenshots (static renders of the boards; light above dark unless the board says otherwise):

- `docs/design/plan-169-model-design/30-model-labels.png`: the label table and the cases
- `docs/design/plan-169-model-design/31-mac-all-agents.png`: Mac All Agents rows and the inspector
- `docs/design/plan-169-model-design/32-mac-inspector.png`: Mac inspector cases and the narrow list
- `docs/design/plan-169-model-design/33-ios-agents-list.png`: iPhone Agents list, light, dark and AX3 text
- `docs/design/plan-169-model-design/34-ios-agent-detail.png`: iPhone agent detail, four cases in light and dark

The board sources are in `docs/design/plan-169-model-design/source/project/` (the same files as the canvas); `source/render.html` renders one board without the canvas runtime and `source/contrast.js` is the contrast check.

## Brief

- **Users:** the Capstan operator, on the Mac and on the phone. Several visits a day. The fast task: "which model is this agent on, and why that one", usually after a plan or task override (`cstan plan model`, `cstan spawn --model`), to confirm the override took effect.
- **Purpose:** show the `model` and `modelSource` members that model-ledger adds to AgentItem. The id says what runs; the source says which layer of the chain (spawn > task > plan > role > host > defaults) chose it.
- **Tone:** calm, exact, native. Axes (1 to 5): task intensity 4 (operate), data density 4, brand expression 1, audience 2 (expert).
- **References (traits only):** Xcode's build-settings levels (a value shown with the level it resolved from, in a quieter colour); Activity Monitor columns (a fixed-width mono-ish column that cuts long values and shows the whole value on hover); Apple Settings "About" rows (label left, value right, long values wrap); GitHub Mobile pull-request lists (a third, quieter metadata line under the title and body); Finder's middle truncation of long file names (the start and the end stay readable).
- **Constraints:** macOS 26 and iOS 26 SwiftUI; SF Pro and SF Mono; light and dark follow the system; Dynamic Type through AX5 on the phone; WCAG 2.2 AA; untrusted host text is never rich text, a link or Markdown. Model ids are 1 to 100 bytes (the ledger CHECK). The host sends no scope id to the app (AgentItem has `model` and `modelSource` only), so no plan or task id is shown with the source. Estimate: 2 hours.
- **Inputs read:** no DESIGN.md, no `docs/design/rules.md` or `directions.md`; `cstan design guide rules`; the Capstan system README and tokens (web look; the apps keep the accent, voice and mono rule and follow macOS and iOS for materials and shapes, as the user decided on 2026-10-28); `docs/design/iphone-app.md`, `apps/macos/docs/pm-visibility-pass.md`, `docs/design/plan-149-design-lists.md`; `AgentRow.swift` (CapstanAppCore), `AllAgentsView.swift`, `AgentsScreen.swift` (the inspector's MetaList), `AgentListRow.swift`, `AgentDetailScreen.swift`; plan-169's model-ledger, model-launch and model-apps packages; `rust/crates/config/src/resolve.rs` for the config sources.
- **Content extremes:** a 100-byte id (`claude-sonnet-5-5-20261015-long-context-preview-eu-west-batch-build-7f3a9c2e1b4d-0a1b2c39c2e1b4d[1m]`); short ids (`gpt-5.1-codex`); ids with a provider prefix (`local/qwen3-coder-32b`); a `[1m]` suffix; agents with no model; a source value this app version does not know.
- **Directions:** none offered. The apps' look is fixed (Glass Workbench on the Mac, Glass Inbox on the phone, both approved), and this package adds one value to existing rows, as plan-149 did. The ui-ux-pro-max pass gave the truncation rule (cut with an ellipsis and offer the whole value) and nothing that changes the approved look.

## The label table

The app never shows `modelSource` as sent. CapstanKit's `ModelLabel` maps it; anything else, including an empty or absent source next to a model, is "other".

| `modelSource` | In a list row (short) | In the inspector and detail (long) | VoiceOver | Means |
|---|---|---|---|---|
| `spawn` | spawn | Chosen at spawn | "chosen at spawn" | `cstan spawn --model` |
| `task` | task | Task override | "task override" | an override on the agent's task |
| `plan` | plan | Plan override | "plan override" | an override on the agent's plan |
| `role` | role | Role setting | "role setting" | the role's model in capstan.toml |
| `host` | host | Host setting | "host setting" | the host's model in capstan.toml |
| `defaults` | defaults | Config defaults | "config defaults" | `[defaults]` in capstan.toml |
| anything else, or absent | other | Other source | "other source" | a source this app version does not know |

Short words in rows because the row has one line of room and the column header already says Model; the long label where there is room to say what the word means. Both are sentence case, as the rest of the apps.

## Where it appears

**Mac, All Agents list (board 31; `AllAgentsView.agentRow`).** A Model column after Role: Agent 132 · Role 112 · **Model 208** · Branch (flexible) · Health. The cell is the id in SF Mono 12 (label colour) then " · " and the short source word in SF Pro 12 (secondary label colour). The source word never truncates; the id truncates in the middle (`.truncationMode(.middle)`), so the family at the start and the version or `[1m]` at the end stay. Hovering the cell shows a tooltip (`.help`) with the whole id and the long label. When the list is narrower than 720 pt (a compact window, or a wide inspector) the Model column is left out and the Branch column keeps its room; the inspector still shows the model (board 32, bottom).

**Mac, agent inspector (boards 31 and 32; the MetaList in `AgentsScreen.content()`).** One row, `Model`, right after `Role`: the whole id in SF Mono, wrapping at any character, then " · " and the long label in the secondary colour. The label stays on one line and moves to the next line when it does not fit. The inspector is shared by All Agents and one project's table, so both get it. **This needs `apps/macos/Capstan/Workspaces/AgentsScreen.swift` in model-apps' owns, limited to that one MetaRow;** the architect agreed and asked the PM to add it.

**iPhone, Agents list (board 33; `AgentListRow`).** A third line under the task line: the id in SF Mono (subheadline size, secondary colour) then " · " and the short word in SF Pro. One line, middle truncation. At accessibility text sizes the id may take two lines, then cuts in the middle; the word moves under it when it does not fit.

**iPhone, agent detail (board 34; `AgentDetailScreen.facts`).** `Model` is the first DetailFact, before Branch (the role is in the title, so the model follows it as on the Mac): the whole id in mono, wrapping at any character, then the long label in the secondary colour on the same line or the next.

## Cases

- **Long id (100 bytes):** rows cut it in the middle (`claude-son…b4d[1m] · spawn` on the Mac, `claude-sonnet-5-…c39c2e1b4d[1m] · spawn` on the phone); the inspector and the detail show all 100 bytes wrapped. The Mac row's tooltip shows it whole.
- **Unknown source:** a value outside the six (a future `fleet`, say) reads "other" / "Other source". The raw value is never shown, so no host text reaches the label.
- **No model:** nothing at all. The Mac Model cell is empty (no dash, unlike Branch), the inspector and the detail have no Model row, the phone row has no third line, and VoiceOver says nothing about a model. This covers an older host that sends no members, an agent registered before the change, and a role with no model.
- **Source without a model:** nothing. **Model without a source:** the id with "other".
- **Empty after cleaning:** the id goes through `HostText.clean` (control characters removed, one line, capped at 100 bytes); an id that is empty after that is no model.
- **Loading and errors:** the model arrives with the agent in the snapshot, so it has no loading state of its own; the list's skeleton and host-offline states are unchanged. With the host offline the last known model stays in the row with the row's existing dimming.
- **Hover and focus:** Mac rows keep the 150 ms `fill2` hover, the 3 px focus ring and the accent selection (board 31: dev-3 hovered with the tooltip, dev-16 focused, dev-14 selected). Nothing on the phone changes.

## Untrusted text

The id is host text: `Text(verbatim:)` after `HostText.clean`, SF Mono, never a link, never Markdown, never in a notification, widget or Live Activity. It is never used to pick a colour or icon. The source label is fixed app text chosen by `ModelLabel`, never the host's string.

## Accessibility

- **Mac row:** the row's accessibility label gains ", model claude-opus-5-5, plan override" (the id whole, then the VoiceOver words from the table). The tooltip is not the only way to the whole id: the inspector has it.
- **Mac inspector and iPhone detail:** the row reads "Model, claude-opus-5-5, plan override".
- **iPhone row:** the combined element (`.accessibilityElement(children: .combine)`) reads the third line as "model claude-opus-5-5, plan override"; the visual "·" is not read.
- **No model:** no words.
- **Dynamic Type:** the phone line follows subheadline; at AX sizes it wraps as above. **Colour:** the id and the source differ by typeface and colour, and the source is a word, so nothing relies on colour.

## Keyboard order

Unchanged: the Model column is not a control. On the Mac the list stays one Tab stop with arrow keys; the inspector's Model row is text. Checked in the mockup on board 31: 12 Tab stops (toolbar, filter, inspector button), each with a 3 px solid ring.

## Kept, moved, changed, removed

- **Kept:** every existing column, row and fact; the per-project Agents table (no new column there; the shared inspector shows the model).
- **Moved:** nothing.
- **Changed:** the Mac All Agents Branch column gives 208 pt to Model (branches already truncate in the middle); the Mac inspector and phone detail gain one row; the phone row gains a third line when there is a model.
- **Removed:** nothing.

## Verification

- **Screenshots:** Playwright rendered the five boards from the canvas files with `source/render.html` at 1900 px wide (paths above). The canvas page itself is private and was not screenshotted. Light and dark are the boards' own `.light` and `.dark` token sets, both rendered. The 375 and 768 px widths do not apply: the Mac boards are a window (narrow case on board 32) and the phone boards are a 402 pt phone; the iPhone SE width was not drawn.
- **Contrast (computed, `source/contrast.js`):** 655 text elements on the five boards, each colour blended over its composited background. All pass AA after one fix: the case labels on board 32 sat on the wallpaper at 3.71:1 and now use the label colour. Lowest per board: 30: 5.97; 31: 4.53 (the light "Working" pill on the selected row); 32: 4.89; 33: 4.98; 34: 4.94. The new id and source text are included in those counts and none is among the lowest four on any board. Backgrounds behind glass are modelled without blur.
- **Keyboard:** Tab traversal on board 31 (above).
- **Not verified:** SF Pro and SF Mono metrics (the render used fallback fonts, so real truncation points differ); real middle truncation (the mockup cuts the text by hand); VoiceOver and Dynamic Type on devices; Liquid Glass; the native build.

## Self-critique

Avoid list: default gradients **pass** (none). Glassmorphism everywhere **pass** (glass only on the existing toolbar, sidebar and tab bar). Emoji or icon grids **pass** (no new icons). Three identical cards in a row **pass**: the three inspector panels on board 32 are three different cases, side by side for comparison. Centred hero **pass** (none). Everything rounded and soft-shadowed **partial**, inherited from the macOS and iOS shapes the user chose. Lorem ipsum **pass**. Generic Inter on white **pass**; Inter stands in for SF Pro only in the mockup.

Require list: two typefaces **pass** (SF Pro, SF Mono). Spacing scale **pass** (4 px; every new value in `model.css` is on it; the 208 pt column is a width, not a spacing). One accent **pass** (no new accent use; the selection and Needs you are the existing ones). Hierarchy and asymmetry **pass** (the id in the label colour, the source quieter; Model sits after Role, not as a heading). Real content **pass** (realistic example data). Motion with meaning **pass** (no motion added). Empty, loading, error, hover and focus states **pass** (Cases above; boards 31 to 34).

Rules: T1 pass, T2 pass, T3 deviation (below), T5 pass, T6 pass (no capitals), C1 pass, C3 pass (computed), C4 pass, A4 pass, P1 pass, P5 pass (no-model and unknown cases stated), P6 pass (sentence case), K5 pass (row height unchanged on the Mac; one extra line on the phone; overflow by middle truncation with tooltip and inspector), K6 pass, K7 pass (the same labels and the same no-model rule on both apps), A1 pass, A3 pass, A6 pass, L8 pass (the Model column leaves narrow lists).

## Rule deviations

- **T3:** 12 px cells and captions on the Mac are the macOS type styles; a larger size comes from the system. Inherited from `mac-app.md`.
- **K1:** concentric radii instead of three fixed radii; the user chose macOS and iOS shapes. Inherited.
- **Design system:** glass and rounded shapes over the web system's "no glass, 4 px"; the user's decision of 2026-10-28.

## Design record changes

No DESIGN.md exists and this package does not own one. Proposed for when one is written: a **resolved value with its source** pattern (the value in mono, " · ", the source in the secondary colour; short word in rows, long label in inspectors); the rule that a host enum is shown only through a fixed app label, with "other" for any value the app does not know; the rule that a missing optional value shows nothing rather than a dash when the column is new.

## Open questions

1. **Short words or long labels in rows?** The boards use "plan", "task" and so on in rows to keep one line. If the user prefers the long labels everywhere, the Mac column needs about 260 pt and the phone line cuts the id sooner.
2. **"Config defaults" vs "Defaults".** The long label names capstan.toml's `[defaults]` (and `[defaults.<kind>]`) as "Config defaults" so it does not read as "the app's default". Confirm the wording.
3. **Scope id.** model-ledger keeps a `scopeId` (the plan or requirement) in `agent_models`, but AgentItem does not carry it. Showing "Plan override · plan-169" would need a protocol member; out of scope here, but worth a later package if the user wants it.
4. **Per-project table on the Mac.** The boards keep the approved per-project table (09) without a Model column; the shared inspector shows it. Add the column there too?
