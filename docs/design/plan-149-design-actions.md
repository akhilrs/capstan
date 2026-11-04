# Capstan for Mac: merge, discard, cancel plan and Messages to PM (plan-149, design-actions)

Status: for the user's approval. This revises approved Mac boards 15 (Merge) and 16 (Messages to PM) and adds two boards (17 Discard integration, 18 Cancel plan). It uses direction A (Glass Workbench) and the tokens of `docs/design/mac-app.md` without change. No implementation code.

- **Design (Claude Design, 4 boards, each light and dark):** https://claude.ai/artifact/FDX39U5s95ABpDmHhX5KRK
- Approved base design (boards 01 to 16): https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8
- Screenshots: `docs/design/plan-149-design-actions/<board>-light.png` and `-dark.png`.
- Board sources (the `.dc.html` artboards and their CSS, for later revisions): `docs/design/plan-149-design-actions/source/`.

## Brief

**Users and task.** Same as `mac-app.md`: the person who runs Capstan and answers it from a Mac in short visits. The tasks that must be fast here are to merge a ready integration into the right branch, to get rid of an integration or a plan that should not go on, and to answer an agent's question without leaving the conversation.

**Purpose.** To bring the Mac to parity with the iPhone app for these actions, with Mac idioms in place of phone ones.

**Tone.** Calm, exact, native. **Axes (1 to 5):** task intensity 4, data density 3, brand expression 2, audience 2 (expert). These are unchanged from the approved record.

**Directions.** Not offered again. The package fixes direction A, which the user chose on 2026-10-28. Re-opening the direction would go beyond the approved boards (package risk 1).

**What was read first (design guide order).**
1. DESIGN.md: none found. `docs/design/directions.md` and `docs/design/rules.md`: none.
2. Design system: the Capstan system (Claude Design) is installed on the canvas, as on the approved canvas. The Mac tokens in `mac-app.md` override its web values, by the user's decision.
3. Brand: rope amber `#8a521c` (light) and `#e0a45e` (dark), as before.
4. Current UI: approved boards 03, 05, 10, 15 and 16, and the iPhone code: `MergeSheet.swift`, `MergeText.swift`, `ItemDetailScreen.swift`, `PlanDetailScreen.swift`, `ConversationScreen.swift`, `ConversationsScreen.swift`, `ReplyComposer.swift` and `ChoiceButtons.swift`.
5. Platform: SwiftUI on macOS 26 (sheets, `.inspector`, `Menu`, popup `Picker`, `.keyboardShortcut`), SF Symbols, light and dark.
6. Content extremes: branch names up to git's limit (truncated in the middle), up to 32 choices of 256 bytes each, a message of up to 4,000 bytes (the user's change of 2026-11-04, which overrides the 500 in plan-149's text), and host text of up to 4096 bytes in untrusted blocks.
7. Accessibility: WCAG 2.2 AA.
8. Scope: four boards and this note. Deadline: the estimate is 3 hours.

**References (traits only).** Xcode's Source Control merge sheet: the branch is a popup and the sheet says what will move. Finder's "Empty Trash" alert: Cancel is the default and the destructive verb is in the button. Messages for macOS: a delivery word under the last bubble, and reply mode shown above the field. Mail's toolbar "More" menu: rare and destructive actions go below a separator. GitHub Desktop's merge dialog: the target branch is chosen in the dialog, with a line that says what happens.

**UX rules applied (ui-ux-pro-max).** Confirmation before destructive actions; a red destructive button kept apart from primary actions; a cause and a recovery in every error; colour never the only signal; keyboard alternatives and escape routes in every sheet; feedback shown while a request is in progress.

## Boards

### 15r Merge sheet (revises board 15)

Mirrors iPhone `8c972757f09d865e7bd3bbf401cf20d969daa9ed` (#64 Merge sheet), `3cb826619f1341a0c3440ba0e00131e3e7aa168e` (Into picker), `2af8302a56b6e692cc2d827799c41c04698c3e29` (#72 fix a refused merge) and the Ask the PM layout in `47837f0fdd8499a9697de4de16ddf41523936aa2` (#80).

States:
1. **Confirm.** Branch, **Into** popup, commit, review and verify pills, and the line "The merge stays on build-host: Capstan sends it to no remote." Cancel has the default focus. The primary button names the target ("Merge into main").
2. **Into menu open.** The branches come from `cstan merge-targets`: the preferred branch first and marked "(checked out, default)". A note under the popup changes with the choice. A checked-out branch reads "the merge updates that checkout, which must have no uncommitted changes". Any other branch reads "the host moves the branch and leaves the checkout as it is".
3. **Loading and fallbacks.** While the list is read, a spinner says "reading the branches on build-host…" and Merge waits. When no branch is listed, or the list cannot be read, the sheet says so and the merge goes into the project's branch. An older host without `merge-targets` shows no Into row.
4. **Results, in the sheet until Done.**
   - Merged, not pushed (green).
   - Conflicts (red): host text with the files, then Ask the PM.
   - Refused because the project is paused (yellow): Resume project, then "The project is resumed" and Merge again.
   - Refused because the working tree has changes (yellow): host text, then Ask the PM. The same shape covers wrong branch, invalid branch, not reviewed, not verified, branch moved, unsafe config and merge failed (red).
   - An integration is already running: Done only.
   - Ask the PM sent: "Open conversation". It goes to board 16r.
   - No answer yet: Retry, which never merges twice.

**What differs from board 15 and why.**
- **The push control is removed:** no "Will push" row, no checkbox, no "Merged, but the push failed" result and no Retry push. The merge verb never pushes (protocol D8). A push would need a separate host change, which is out of scope (package risk 2).
- **Into is now a choice** (a popup button) instead of a fixed "project's HEAD branch". On the Mac it is an `NSPopUpButton`-style `Picker` in the sheet, where the iPhone uses a navigation-link picker.
- **Results stay in the sheet** until Done, because the follow-up actions (Resume project, Merge again, Ask the PM) belong to the same decision. Board 15 put them in the inspector. After Done, the inspector shows the last result as a banner, as on board 04.
- **The refusal set follows the host codes** (`docs/remote-protocol.md`, merge codes), with the iPhone's fixes. "Message PM…" becomes "Ask the PM", which sends a prepared message.
- **Keyboard:** Esc and Return cancel, because Cancel is the default on a sheet that changes a branch. Merge needs a click, or Tab and Space. The item menu gets Merge… with ⇧⌘M (⌘M stays Minimize).

### 17 Discard integration (new)

Mirrors iPhone `5cee735188be62c93f784d91604a45dd071428bd` (#81).

States:
- **The action:** the integration inspector's More menu (and the Item menu in the menu bar) has "Discard integration…" (⌘⌫). It sits below a separator, away from the primary Merge… button. It shows only when the integration is ready, the host is live and the host offers `integration discard`.
- **Confirm sheet:** "Discard this integration?" with the iPhone's wording, the integration, branch and commit, the summary in an untrusted mono block, Cancel as the default focus and a red-filled "Discard". There is no note field, as on the iPhone.
- **Results:** Discarding… (progress), Discarded (the row leaves Waiting for you), No longer waiting, and Could not reach the host with Retry.

### 18 Cancel plan… / Cancel draft… (new)

Mirrors iPhone `9bbd75cc4e0f86a50e9ea24cce0978a14ab271fe` (#82).

States:
- **The action:** the plan inspector's More menu (and the Plan menu) has "Cancel plan…", or "Cancel draft…" for a draft, with ⌘⌫. There is no menu when the host cannot decide or sent no binding for the plan.
- **Confirm sheet:** "Cancel this plan?", the iPhone's footnote, the plan, its state, and the packages and developers that stop, with the title in an untrusted block. The dismiss button is **"Keep plan"** (or "Keep draft"), not "Cancel", so that "Cancel" and "Cancel plan" never stand side by side. This is a Mac wording change; macOS alerts name both choices by their verbs. Keep plan is the default focus.
- **Sending:** "Cancelling…" with a spinner, and both buttons off.
- **Cancelled state:** the inspector shows "Plan cancelled. Its open packages were stopped and the PM was told." The plan moves to the Done filter with a grey "Cancelled" pill with a glyph.
- **Other answers:** Not waiting, This changed since you opened it, and Could not reach the host with Retry.

### 16r Messages to PM (revises board 16)

Mirrors iPhone `f3a369a0a2ed062119c688364b4ebc024785858b` (#75 questions answered in place), `fc78ec959d62c2980051f8a435ae9af3a55c1ebf` (#77 choices as buttons) and `47837f0fdd8499a9697de4de16ddf41523936aa2` (#80 delivery state).

States:
- **Delivery line** under each sent message, with a word and a glyph: Sending; Sent; Queued ("the PM gets it when it is free", and after a while "Waiting for the PM for N min: it is busy, stuck at a question, or paused"); Delivered; Read; Not sent · reason with Retry (the text is kept). A message the host dropped reads "Not delivered (state)".
- **An agent's question in the thread:** the sender, the time and a "Question" pill, the text in an untrusted mono bubble with an accent outline, the choices as buttons (⌘1 to ⌘9), and Reply (⌘R).
- **Reply mode:** a bar above the composer reads "Replying to architect-3's question", with a close button and Esc to go back to a new message. The field and the button change to "Send reply". If the question is answered elsewhere, a banner says so and the composer goes back to new messages; the typed text is kept.
- **Choices fill the composer** and never send by themselves. They are in the thread and in the inspector.
- **Return sends** and ⇧Return adds a new line. A byte count shows under the field ("28 of 4,000 bytes"), and near the limit it says how many are left ("3,720 of 4,000 bytes · 280 left"). The field grows to 8 lines, then scrolls. Send is off while the field is empty.
- **Over the limit:** the field gets a red outline, and an error line says "4,112 of 4,000 bytes · 112 over. Shorten it to send; Return does nothing until then." Send is off. The text is never cut silently.
- **Unread count:** "Messages to PM" in the sidebar shows the total, and each conversation shows its own count. A conversation with unread messages has a bold name.
- **Notification:** "Question on payments-api: architect-3 asked you a question." It carries no host text.
- The inspector shows the selected question with its choices; "Answering here, in the thread or in Waiting for you answers the same question once."

**What differs from board 16 and why.**
- The delivery states go from four (Sending, Sent, Delivered, Not sent) to six plus dropped (adding Queued and Read), because the host now reports them (#80).
- Questions are answered in the thread with Reply mode (#75). On board 16 they were only shown, and answered from the inbox.
- Choices are buttons (#77), with ⌘-number shortcuts on the Mac.
- Return sends (a Mac chat idiom, as in Messages). The iPhone sends with a button.
- The limit is 4,000 bytes, where board 16 had 500 (the user's change), with a near-limit count and an over-limit state.
- The compose popover (⇧⌘N) and the project picker are unchanged from board 16 and are not redrawn.

## Untrusted text

Every string from the host is shown in a flat mono block or mono bubble with a labelled header and no links: commit summaries, plan titles, the conflicting files, git's reasons and agent messages and questions. Notifications name the project and the role only. The message that Ask the PM writes is the user's own text and quotes host text only as plain text.

## Verification

- **Screenshots:** 8 PNGs (4 boards × light and dark), rendered with Playwright from the board sources. A local flattener expanded the canvas's light and dark loop. Files: `15r-merge-*.png`, `17-discard-integration-*.png`, `18-cancel-plan-*.png`, `16r-messages-to-pm-*.png`.
- **Contrast (computed in the page, not by eye):** each text element's colour was blended over its composited background chain. Every element passed AA (4.5:1). Disabled controls are excluded, as WCAG allows. So is the content dimmed behind a sheet's scrim, which is `aria-hidden`.

| board | light: elements, lowest | dark: elements, lowest |
|---|---|---|
| 15r Merge | 94, 4.62:1 ("Review passed" pill) | 94, 5.44:1 |
| 17 Discard | 38, 6.06:1 | 38, 5.40:1 |
| 18 Cancel plan | 49, 5.05:1 ("Cancelled" pill) | 49, 5.06:1 |
| 16r Messages | 89, 4.89:1 ("Question" pill) | 89, 4.82:1 (sidebar count) |

  Control labels (buttons, menu items, chips) are in these counts. The focus ring is the 3 px blue `focus` token.
- **Keyboard:** Tab was traversed on all four boards in light. Every stop showed the focus ring: 22 stops on 15r, 8 on 17, 9 on 18 and 40 on 16r. The sheets put Cancel or Keep first in focus as the default. In the mockup the side-by-side states are separate stops. In SwiftUI a sheet traps focus.
- **Widths:** a Mac window has no 375 or 768 px layout, so the 375 and 768 px checks do not apply, as recorded for `mac-app.md`. The compact window behaviour of board 06 is unchanged.
- **Not verified:** real Liquid Glass (the HTML blurs instead); SF Pro metrics (Inter stands in); VoiceOver announcement of the delivery line; contrast behind glass with real wallpaper (glass is modelled without blur); the native build.

## Self-critique

Avoid list:
- Default gradient: **pass**.
- Glassmorphism everywhere: **pass** (glass only on sheets, menus, the composer and the notification).
- Emoji or icon grids: **pass**.
- Three identical cards: **pass** (the result panels each hold a different state and action).
- Centred hero: **pass**.
- Everything rounded and soft-shadowed: **partial**, as in the record (macOS concentric radii and floating shadows).
- Lorem ipsum: **pass**.
- Generic Inter on white: **pass** (Inter stands in for SF Pro).

Require list:
- Two typefaces: **pass**.
- Spacing scale (4 px): **pass**.
- One accent: **pass**. Amber is used for primary buttons, selection and reply mode. Destructive buttons are red, not the accent.
- Hierarchy and asymmetry: **pass**.
- Real content: **pass** (example data, generic names).
- Motion with meaning: **pass** (spinners only while sending; no new motion).
- Empty, loading, error, hover and focus states: **pass**, with one exception. Loading, error and focus are on each board and empty is the cancelled plan. Hover is the highlighted menu item; the row hover from board 06 applies unchanged and is not redrawn.

Rules: T7 tabular figures **pass** (counts and times). K3 action hierarchy **pass** (one primary per sheet). A4 not colour alone **pass**. P3 verb buttons **pass** ("Keep plan", "Discard", "Merge into main").

## Rule deviations

- **T3 and T4 and K1:** the same as `mac-app.md`, which uses macOS type styles and concentric radii.
- **A1, disabled controls:** disabled Send and Keep draft are at 45 % opacity and below 4.5:1. WCAG exempts inactive controls; this is an accepted trade-off.
- **Highlighted destructive menu item:** a highlighted "Cancel draft…" uses the system's accent highlight, not red, because macOS draws the highlight; the item reads red when it is not highlighted.

## Design record changes

`docs/design/mac-app.md` is not changed by this package. When the record is next updated, it should take these changes:
- Screen 15: replace the push row, the push checkbox, "Merged, but the push failed" and Retry push with the Into picker and the result set above. Results stay in the sheet until Done. Remove open question 2 ("Push default"); the merge never pushes.
- Screen 16: messages may be up to 4,000 bytes (was 500), with near-limit and over-limit states; the delivery states become Sending, Sent, Queued, Delivered, Read and Not sent (and Not delivered); Reply mode, choice buttons, Return sends.
- New screens 17 (Discard integration) and 18 (Cancel plan / Cancel draft).
- Open question 1 (protocol work): the merge, discard, plan-cancel and message verbs now exist in the protocol.
- Components: add the popup button with its menu, menu item (normal, highlighted, destructive, shortcut), choice chip, reply bar and delivery line. Sheet rule: a sheet that would show "Cancel" next to "Cancel <thing>" names the dismiss button "Keep <thing>".
- Keyboard: ⇧⌘M Merge…, ⌘⌫ Discard integration… or Cancel plan…, ⌘R Reply, ⌘1 to ⌘9 choices, Return sends, ⇧Return new line, Esc leaves Reply mode.

## Open questions

1. **Merge while the branch list loads.** Merge waits for the list, as drawn. Should it instead allow merging into the project's branch at once, with no Into row, after a timeout?
2. **"from my Mac".** Ask the PM writes "merging … from my Mac was refused". Is that wording right, or should both apps say "from the app"?
3. **⌘⌫ for destructive actions.** It is the Finder idiom for "Move to Trash". Should Discard and Cancel plan have no shortcut, so that they are only ever reached by the menu?
4. **Choice shortcuts.** ⌘1 to ⌘9 cover nine choices; the protocol allows 32. Choices after the ninth have no shortcut. Is that acceptable?
