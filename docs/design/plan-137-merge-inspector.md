# Merge inspector: purpose, impact, results and next steps (plan-137, inspector-design)

Status: design for the user's approval. app-details waits for it.

Claude Design: https://claude.ai/artifact/Gn1qjVH1dA81Xy4ahvtFuc (canvas "Capstan for Mac — Merge inspector (plan-137)", artboards 17, 18 and 19, each in light and dark).

It extends the approved Glass Workbench design (https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8, `docs/design/mac-app.md`). It uses the same `base.css` and tokens, and the Capstan design system is installed as the record. The approved canvas is unchanged. The merge sheet (15, plan-130 ui-merge) is not redesigned.

Screenshots (static renders of the artboards, light above dark):

- `docs/design/plan-137-merge-inspector/17-built.png`
- `docs/design/plan-137-merge-inspector/18-conflicted.png`
- `docs/design/plan-137-merge-inspector/19-states.png`

## Brief

- **Users:** the one Capstan operator, on a Mac, checking an integration between other inbox items. It happens several times a day. The fast task is "what is this, is it safe, what do I do now".
- **Purpose:** the integration inspector says what the integration is for, its impact and the next step. It never shows a bare "merged" for an integration that is not in main yet (the user's words, through the PM).
- **Tone:** exact, calm, plain. Axes: task intensity operate, data density medium-high, brand expression neutral, audience expert.
- **References (traits only):** GitHub pull request merge box (the merge state is the first thing you read, with checks beneath it); Xcode Issues inspector (grouped sections that fold, with counts); Tower's merge view (the branch and target named before the action); `git diff --stat` (files changed, insertions and deletions as the impact summary); macOS Finder Get Info (fold-out sections with a summary line).
- **Constraints:** macOS 26 SwiftUI `.inspector`, 360 px column; Liquid Glass rules 1 to 7 from `mac-app.md` (no glass on inspector content); SF Pro and SF Mono; light and dark follow the system; WCAG 2.2 AA; untrusted text is never rich text, never a link; the frozen field list below. Deadline: none given; the estimate is 3 hours.
- **Inputs read:** no DESIGN.md; no `docs/design/rules.md` or `directions.md`; Capstan system via the approved canvas's install; `mac-app.md` tokens; screens 05 and 15; `docs/reference/workflow.md` (`integrate confirm`); architect-54's answer on plan-130 (below).
- **Directions:** none offered. The direction (A, Glass Workbench) was picked and approved for the whole app, and this package extends it.

## Screens and states

| Artboard | Screen | States |
|---|---|---|
| 17 | Built, not in main yet: (a) terminal path with full details, (b) once plan-130's Merge button exists, sections folded, (c) after an app merge, with the "merged but not recorded" exception | focus on Copy commands; hover on a folded section; every section filled |
| 18 | Conflicted, details not recorded; the state words in inbox rows | focus on Message PM…; selected row |
| 19 | Built before this change (recorded false); loading; details null (host too old); impact truncated, areas and migrations omitted; impact null with timeout and with diff_failed; review findings; verify not run on this commit, with latest and lastPassed; review null; verify null; every verdict and verify state; textVerified false; truncated text; plans, reports and signoffs omitted; a sign-off whose plan is not in plans | loading, empty, error |

Hover and focus follow `mac-app.md`: 150 ms hover fill (`fill2`), 3 px `focus` ring. The sections fold with the system disclosure animation, and only the chevron moves under Reduce Motion. Nothing animates on its own.

## Section order and labels

1. Header: eyebrow "Integration · <project>", title, state pill, then Branch, Into, Head.
2. **What it is for**: plans, architect sign-offs, reports.
3. **Impact**: files changed, insertions and deletions, areas, migrations.
4. **Results**: review, verify.
5. **Next steps**.

Sections 2 to 4 are disclosure groups with a summary line ("plan-12 · 3 reports · signed off", "23 files · +1,284 −96", "review passed · verify passed"). They open by default on the terminal path. Once the Merge button exists they start folded, so the button sits high in the column.

## State wording

| `integrationState` | Pill | Title | Lead |
|---|---|---|---|
| `merged` | blue, clock glyph: **Built · not in main** | "<branch> is built, not in main yet" | "Its N reports merged cleanly into the integration branch. <HEAD> does not have them until the branch is merged." |
| `conflicted` | yellow, warning glyph: **Conflicted · nothing to merge** | "<branch> has a conflict" | banner "The reports conflict with each other: Building the integration branch stopped at a conflict. Nothing reached main and nothing is ready to merge." |
| after an app merge (plan-130) | green, check: **In main · confirmed** | "<branch> is in main" | banner "Merged into main and confirmed: <HEAD> is now <sha> · <time>. pm-1 was told." |

The bare word "merged" never appears on any screen: not in pills, rows, titles or notifications. The feed's `merged` means "merged into the integration branch". Inbox rows use the same pill words (artboard 18).

## Field table

Every text value (`title`, `summary`, `path`, `migrations[]`, `nextSteps[]`) carries `textVerified` and `truncated`:

- `textVerified` false: the text is replaced by "<Title/Summary/Path/Step> not shown: it failed its integrity check." with a warning glyph. The summary form adds "Read it in the terminal."
- `truncated` true: the text ends with "…", and "Cut by the host. The full <kind> is on the host." follows.

The untrusted style is the app's untrusted-text block: a flat mono block (`mono-bg`, hairline, radius 10) with a labelled header "<Kind> · untrusted <text|title|summaries|paths>". It is never glass, never a link and never rich text. IDs (`planId`, `reportId`, `packageId`, `reviewerAgentId`) and SHAs are trusted. They are set in the label2 caption above each untrusted line, or in mono in the meta list.

| Field | Label | Format | Absent / null text |
|---|---|---|---|
| `integrationState` | header pill | see State wording | not null for an integration item |
| `branch` | Branch | mono | not null |
| `headSha` | Head | mono, 12 characters | not null |
| `integrationDetails` | (whole body) | sections 2 to 5 | null and still loading: skeleton bars under the header; null from the feed: "This host's Capstan does not send integration details. Update Capstan on <host> to see purpose, impact and results. The next steps below still apply." |
| `recorded` | (whole body) | true: sections 2 to 4 | false with `conflicted` (normal): section heading "What it is for · Impact · Results" and "Not recorded. Capstan records these only when an integration merges cleanly; a conflicted one never has them. This is expected." false with `merged` (built before this change): grey banner "Details not recorded: This integration was built before Capstan kept its purpose, impact and results. Run `cstan integrate show` on the host to see its reports." |
| `plans` | Plan(s) · untrusted title(s) | one line per plan: `planId` caption, `title` mono | [] : "No plan: these reports were integrated outside a plan." |
| `plans[].planId` | caption | `plan-12` | not null |
| `plans[].title` | mono line | untrusted text | not null; see textVerified, truncated |
| `plansOmitted` | last line of the block | "and N more plans" (not a button) | 0: no line |
| `reports` | Reports · untrusted summaries | caption "`reportId` · `packageId`", then `summary` in mono | [] : "No reports listed." |
| `reports[].reportId` | caption | `r-118` | not null |
| `reports[].packageId` | caption | `search-index`; a report outside a plan shows "no package" | null: "no package" |
| `reports[].summary` | mono line | untrusted text | not null; see textVerified, truncated |
| `reportsOmitted` | last line | "and N more reports" | 0: no line |
| `signoffs` | Architect sign-off · untrusted text | caption "`planId` · `createdAt`", then `summary` | [] : "Not signed off yet." |
| `signoffs[].planId` | caption | `plan-15`; a plan not in `plans` adds "· not in this integration's plans" | not null |
| `signoffs[].summary` | mono | untrusted text | not null; see textVerified, truncated |
| `signoffs[].createdAt` | caption | "today 12:40", "yesterday 17:05", "Oct 24" | not null |
| `signoffsOmitted` | last line | "and N more sign-offs" | 0: no line |
| `impactError` | Impact | grey banner | `timeout`: "Impact not measured: the diff timed out. The host stopped measuring the diff after its time limit. The merge itself is not affected." `diff_failed`: "Impact not measured: git could not diff. git failed to compare the branch with main on the host. Run `git diff --stat <HEAD>...<branch>` there to see it." null: no banner |
| `impact` | Impact | stat line, areas block, migrations line or block | null with `impactError` null (recorded true): "Impact not recorded." |
| `impact.filesChanged` | stat | "23 files changed" (15 px, 600, tabular) | not null |
| `impact.insertions` | stat | "+1,284" in green, the sign carries it | 0: "+0" |
| `impact.deletions` | stat | "−96" in red, the sign carries it | 0: "−0" |
| `impact.truncated` | stat | "At least N files changed", then "The diff was too large to count in full; these are the counts up to where the host stopped." | false: plain counts |
| `impact.areas` | Areas · untrusted paths | rows: `path` mono (one line, ellipsis), `files` count right-aligned, tabular | [] : no block |
| `impact.areas[].path` | row | untrusted text | not null |
| `impact.areas[].files` | row | integer | not null |
| `impact.areasOmitted` | last row | "and N more areas" | 0: no row |
| `impact.migrations` | Migrations · N · untrusted paths | database glyph in the header, one mono path per row | [] : "Migrations: none." |
| `impact.migrationsOmitted` | last row | "and N more migrations" | 0: no row |
| `impact.nextSteps` | Next steps (host steps) | each step is a numbered step holding a "From the host · untrusted text" block | [] : no host steps; only the app's own steps |
| `review` | Review | pill + "`reviewerAgentId` · round `round`" | null: grey pill "Not reviewed yet" |
| `review.verdict` | pill | `passed` green "Passed" ✓; `findings` yellow "Findings" ⚠; `failed` red outlined "Failed"; `cancelled` grey "Cancelled" | — |
| `review.reviewerAgentId` | after the pill | `reviewer-1` | not null |
| `review.round` | after the pill | "round 2" | not null |
| `verify` | Verify | see the next rows | null: grey pill "Never run" + "no verify has finished for this branch" |
| `verify.latest` | Verify | if `commitSha` = `headSha`: pill + "on this commit · <age>"; otherwise a grey pill "Not run on this commit" and a note "Latest: <pill> on <sha> · <age>" | not null when `verify` is set |
| `verify.latest.state` | pill | `passed` green "Passed"; `failed` red "Failed"; `timeout` red "Timed out"; `error` red "Error" (red pills outlined) | — |
| `verify.latest.commitSha` | note | mono, 12 characters | — |
| `verify.latest.finishedAt` | note | "2 h ago", "3 h ago", "yesterday 17:05" | — |
| `verify.lastPassed` | note | "Last passed on <sha> · <time>"; hidden when it is the same run as `latest` | null: no line |
| `verify.lastPassed.commitSha` | note | mono, 12 characters | — |
| `verify.lastPassed.finishedAt` | note | as `finishedAt` | — |
| `textVerified` | any text value | true: shown | false: "<Kind> not shown: it failed its integrity check." |
| `truncated` | any text value | false: shown in full | true: "…" plus "Cut by the host. The full <kind> is on the host." |

The feed never sends a started review, or a verify run that is queued, running, cancelled or interrupted. They are not shown. The Results section says so under its rows: "Finished results only. A review in progress and a queued or running verify are not shown."

## Next steps

Numbered steps. The next one has an accent number; the others are grey.

**Built · not in main, before plan-130 (terminal path):**

1. **Merge it into main.** "In the project's checkout on <host>:", then a trusted command block built by the app, with the primary button **Copy commands**:
   ```
   git switch <HEAD branch>
   git merge <branch>
   ```
2. **pm-1 confirms the merge.** "Tell pm-1 it is in main. pm-1 runs `cstan integrate confirm`, which removes the branch." Button: **Message PM…**.
3. Then each of `impact.nextSteps`, in order, as an untrusted block "From the host · untrusted text".

**Built · not in main, once plan-130's Merge button exists:**

1. **Merge it into main.** "Opens the confirmation sheet. After the merge Capstan confirms the integration itself and tells pm-1; no confirm step is left." Primary **Merge…**, and a plain **Terminal commands instead** that unfolds the commands from the terminal path.
2. Then each host step.

After the app merge, the header becomes **In main · confirmed** and the steps are only the host steps. Exception (the merge answered `merge_unrecorded`, or the settle failed and the integration is still `merged`): a yellow banner "In main, not confirmed: The merge worked but Capstan could not record it." Step 1 becomes "pm-1 confirms it by hand with `cstan integrate confirm`", with **Message PM…**. architect-54 confirmed this flow (2026-10-29).

**Conflicted:** there is no merge step.

1. **pm-1 has a developer resolve it.** "The fix arrives as a new report. Nothing to do here unless you want to tell pm-1 something." Button: **Message PM…**.
2. **A new integration replaces this one.** "When it is built, it shows up as its own item with its purpose, impact and results. This item then leaves the inbox."

**recorded false, built before this change:** the terminal steps 1 and 2, with no host steps.

## Light and dark

Both come from the `mac-app.md` tokens and nothing new: blue, yellow and green pills on their tints, red pills outlined, and the accent on the primary button and on the next step's number only. Dark re-tunes the accent to `#e0a45e` and the state colours as the tokens say.

## Verification

- **Screenshots:** Playwright rendered the three artboards from static copies of the canvas files at 1360 px, with light and dark stacked on each board. The canvas page itself is private and was not screenshotted.
- **Contrast (computed):** a script blended each text element's colour over its composited background on all three boards. 536 elements were checked; all pass AA, and the lowest is 5.18:1. Backgrounds behind glass are modelled without blur.
- **Keyboard:** pressing Tab follows reading order: Copy commands → Message PM… → the section disclosures → Merge… → Terminal commands instead. Every stop shows the 3 px `focus` ring (`#1d5bb8` in light). In SwiftUI the list itself is one stop.
- **Widths:** 375 and 768 px do not apply to a Mac inspector, as in `mac-app.md`. The panels are drawn at the inspector's width.
- **Not verified:** real Liquid Glass and SF Pro metrics (Inter stands in); VoiceOver; Dynamic Type; the user's look in Claude Design.

Avoid list: default gradient **pass**; glass everywhere **pass** (no glass in the inspector); emoji or icon grids **pass**; three identical cards **pass** (the three panels on 17 are three different states); centred hero **pass**; everything rounded and soft-shadowed **partial**, as in `mac-app.md` (concentric radii, shadow only on the panel frame of the mockup); lorem ipsum **pass**; generic Inter on white **pass** (SF Pro is the face; Inter only stands in).

Require list: at most two typefaces **pass** (SF Pro, SF Mono); spacing scale **pass** (4 px); one accent **pass**; hierarchy and asymmetry **pass** (state first, next step marked with the accent number); real content **pass** (realistic example data); motion with meaning **pass** (disclosure only); empty, loading, error, hover and focus **pass** (artboards 17 to 19).

## Rule deviations

- **K1:** the radius family is concentric (16, 10, 8, capsule), as the user decided for the Mac app.
- **K2, C5 and the design system's "no glass, 4 px radii":** inherited from the approved Mac design (the user's decision), not new.
- **T3:** captions and mono are 12 px in a dense inspector. This is the `mac-app.md` type scale, and C3 passes.
- **L8:** no 375 or 768 px layouts, because a Mac inspector is a fixed column.
- **K6:** disabled and loading states for Copy commands and Message PM… are not drawn. They are the standard button states from artboards 04 and 06.

## Design record changes

None. DESIGN.md does not exist and this package does not own it. `mac-app.md` and its tokens are unchanged. The proposed additions for a future record: the state words "Built · not in main", "Conflicted · nothing to merge" and "In main · confirmed"; the rule "never a bare 'merged'"; the untrusted list block (caption plus mono line, "and N more", "Cut by the host"); the trusted command block (`fill2`, mono, no untrusted header).

## Kept, moved, changed, removed

- **Kept:** the 05 merge card's Branch, Into and Head meta (Head was "Commit"), and Review and Verify with pills.
- **Moved:** Review and Verify go into the Results section. The plan line becomes "What it is for".
- **Changed:** the title "is ready to merge" becomes "is built, not in main yet". The "Packages 3 of 3 integrated" line is dropped, because the frozen list has no package count; the report count gives it.
- **Removed:** "Open plan-12" (not in the frozen list; app-details may add it later with a new field).

## Open questions

1. The commands use `git switch <HEAD branch>` then `git merge <branch>` (a merge commit). Should the inspector suggest `--ff-only` or `--no-ff`, or leave the choice to the user?
2. Should the sections fold on the terminal path too, opening only "Next steps", once the user has seen several integrations?
3. "Open plan" was dropped because `plans[]` carries no route. Should the details feed add one in a later plan?
