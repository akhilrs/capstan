# Project overview: proposal (phase 1 of 3, revision 3)

Status: revised after external-review-23 (on 53404be) and its second look (on 5064f4b). No Claude Design yet. Next: a short second review, then
artboards in Claude Design under the approved Mac app design (https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8,
direction A Glass Workbench, `docs/design/mac-app.md`). Section 6 answers each review point.

**Problem.** When a project is selected and nothing waits, the list column shows only the empty state from
`InboxScreen.swift`: a tray icon, "Nothing is waiting for you" and a host count. The app already holds the project's
agents, plans and run, but shows none of them. The user can't tell a busy project from a stuck, paused or finished one.

**Proposal.** Add a project overview to the list column for a selected project. It has three parts: a compact status
strip that always sits at the top, the waiting groups when there are any, and the details below them. The overview
reports facts the host sent and doesn't predict anything. Nothing waiting is not the same as all good: the strip names
the project's state.

## 1. Questions the view answers at a glance

| # | Question | Answered by |
|---|---|---|
| Q1 | Is anything moving? | status strip: project status word and agent counts |
| Q2 | Who is doing what? | Agents now |
| Q3 | What is next, and when do I come in? | Plans in progress (facts); the waiting groups say what needs you now |
| Q4 | Is something stuck? | status strip counts (Stalled, Blocked, Lost), exceptions first in Agents now |
| Q5 | What changed while I was away? | Recent outcomes (bounded backfill) |
| Q6 | Can I trust what I see? | freshness line: connection, project availability, snapshot completeness |

## 2. Freshness model (applies to every section)

The overview keeps three signals apart and never mixes them:

1. **Connection liveness (host-wide).** The app's local clock time when it last received any line on the host's stream.
   `heartbeat` is stream-wide, so it never refreshes a project's own age. This uses the approved rule: "no heartbeat"
   after 3 missed poll intervals. It is shown in the existing status capsule (screen 11).
2. **Project availability.** The `state` of the project's `project` line (`ready`, `silent`, `refused`, …), worded as
   on screen 07.
3. **Project data completeness.** From the first `snapshot` line of a connect or `resync` until `snapshotEnd`, the
   project is **Refreshing** and every count is shown as unknown ("–"), not as a stale number. After `snapshotEnd` the
   line says "current as of 14:05". That time is the local receipt time of the project's last line, `snapshotEnd` or an
   `event`.

**Receipt time is not occurrence time.**
- **Receipt time** (local clock) is used only for freshness: "current as of".
- **Transport timeouts** (the 3-interval rule, reconnect backoff) use monotonic elapsed time, never wall clock.
- **History rows** show when the event *happened*: the event's host `ts`, labelled "host time" in the inspector. The
  row age uses the existing skew-aware rule from plan-132's `activitySince`:
  - a future time (skew) shows the absolute host time with no age;
  - older than 24 h shows the date ("yesterday, 18:40").

  A report from yesterday that arrives by backfill reads "yesterday, 18:40", never "just now".

**"Current as of" never advances on an invalidation alone.**
- On `resync`, or a reconnect that sends a snapshot, the project enters **Refreshing** at once. This includes a snapshot
  with no items.
- An `invalidate` effect marks only its item **pending**. That item keeps its last-known value, dimmed, until its
  `feed item` read succeeds.
- "Current as of" moves only when every pending read and the snapshot have arrived.
- **Failed refresh:** when a read fails or the stream ends mid-snapshot, the project shows "Refresh failed at 14:06 ·
  showing last known from 14:02". It retries on the normal backoff and offers a Retry button.

When a project is not `ready`, or the connection has missed 3 intervals, every section dims and gets the label "last
known, 14:02".

## 3. Sections

The sources are v1 snapshot items, `project` lines and events (`docs/remote-protocol.md` sections 2, 4 and 5).
"Client only" means the value comes from data the app already receives.

### 3.1 Status strip (must; always at the top)
- **Shows:** one status word, the counts, and the freshness line:
  `Moving · 3 working · 2 idle · current as of 14:05`, or
  `Needs attention · 3 working · 1 stalled · 1 lost · 2 idle`.
- **Status words**, the first that holds:
  - Unknown: refreshing, refresh failed, or not `ready`.
  - Paused: `RunItem.paused`.
  - Needs attention: any agent Lost, Blocked or Stalled.
  - Moving: any agent Working. Needs you alone does not count as Moving; those agents wait for the user and appear in
    the waiting groups.
  - Quiet: none of the above.
- **No "Completed" and no "No work assigned".** v1 has no authoritative completed state, and integrated package counts
  don't prove a merge. Both words could also hide each other in the normal finished case. Quiet is followed by a fact
  line instead, built from counts: "No plan in progress · 2 plans integrated" or "No active agent has a task". Later:
  a "Completed" word, if the host ever sends a merged state.
- **Counts:** every health word of plan-132, Lost included. A count of 0 is left out. Unknown counts show "–".
- **Source:** client only. `AgentItem`, `RunItem` (`state`, `paused`), `PlanItem`, and attention from
  `plan.needs_attention`. The run has no pause actor or time in v1, so the strip says "Paused", never "paused by you at
  11:58".

### 3.2 Agents now (must; capped)
- **Order:**
  1. Exceptions (Lost, Blocked, Stalled, Needs you), at most 4. More shows "+3 more need attention" and joins the
     View all row.
  2. At most 3 Working agents, by `activitySince`, oldest first.
  3. One row "View all 8 agents ›", which opens screen 09.
- **Shows:**
  - The agent id and role.
  - The task: one line of untrusted text.
  - The health pill with its plan-132 word, symbol and colour.
  - With plan-132, the age "working for 6 min", computed locally from `activitySince` and left out under clock skew
    (the plan-132 rule).
- **Branch is shown only when the field is present.** It is in the host code's `AgentItem` as an additive field (v1
  section 7 allows that), but the v1 document doesn't list it. Required protocol work: document `branch` on
  `AgentItem`. Until then the app treats it as optional.
- **Source:** `AgentItem`, plus the plan-132 `activity`, `activitySince` and `activitySeq`. These are on
  `feat/PM-354-…` (ea50f1d), not yet on main. Without them, the plan-132 estimate rule applies.

### 3.3 Plans in progress (must; facts only)
- **Shows:** every plan in state `submitted`, `in_review`, `approved`, or `signed_off` and not yet merged. A plan
  leaves the section on `integration.finished` for its integration, or on a state outside that set. Each row has its
  title, the existing progress bar, and fact lines built only from its counts and state: "2 unassigned · 1 reported,
  not integrated · 3 integrated of 6", "signed off, not merged", "submitted for approval".
- **No predicted next actions:** no "next: PM signs off", no "who" and no "when".
- **Source:** `PlanItem` (`state`, `packages {total, assigned, reported, integrated}`). "Unassigned" is total minus
  assigned. Client only.

### 3.4 Recent outcomes (must; bounded backfill)
- **Shows:** by default, the last 3 to 5 outcomes for the selected project. Show more goes up to 20 lines. The
  outcomes:
  - `report.recorded`: "dev-4 reported a03f9c1". The event has `commitSha` and no branch.
  - `review.verdict`: "review-12 passed". The event has no branch.
  - `verify.result`.
  - `integration.finished`.
  - `plan.approved` and `plan.signed_off`.
  - `design.decided`.
  - `agent.lost` and `agent.blocked`.
  - `finding.raised`.
- **Dropped:** routine state changes such as `snapshot.invalidated`, `message.state` and `agent.registered`.
- **Why must:** it answers "what changed while I was away?" the first time a project is opened.
- **Required protocol work: `feed recent`.** It is a read-only route over a fixed `{epoch, head}` that the client
  passes. Today it can't be built from `feed page`: without an anchor, only `--after 0:0` works. Its budgets:
  - At most 20 events returned.
  - At most 500 ledger rows scanned.
  - A response of at most 48 KiB.
  - It answers `truncated: true` when a budget stops it.
  - It is only for the selected project, once per project per connection, plus an explicit Refresh.
  - It is never called per poll. The host rate-limits it per client.
- **Client rules:**
  - The backfill is display-only and is held in its own store. It never goes through the reducer, never applies
    effects, never triggers notifications and never moves the cursor.
  - The client de-duplicates by `projectId + epoch + seq + k` against the live events.
  - A change of epoch or a `resync` drops the backfill.
- **Rows show occurrence time** (section 2). Backfilled rows and live rows look the same and are ordered by `seq`.
- **History states** (each drawn):
  - **Loading:** 3 skeleton rows.
  - **Backfilled:** "Last 5 outcomes".
  - **Session only:** the host has no `feed recent` (it answers `unknown_command` or `invalid_request`). This is an
    explicitly reduced capability: "This host can't show earlier history. Showing what happened since you connected at
    14:02."
  - **Error:** "Couldn't load earlier history" with Retry. Live rows still show.
  - **Truncated:** "Earlier history not loaded: the host stopped after its scan limit". A bounded scan with no outcomes
    says "No outcomes in the last 500 events". It never says "nothing happened".
  - **Gap:** after a `resync` (new epoch or head), the app fetches again at the new head. Until that read succeeds, a
    divider shows "History before 14:06 may be missing".

### 3.5 Project actions (should)
- **Shows:** in the inspector when no row is selected:
  - Open plans and Open agents.
  - Pause project… or Resume project…, with the existing sheet and `cstan decide run pause|resume`.
- **Message PM appears only when a `message:` item waits**, as `message reply`. v1 has no free message to the PM, and
  screen 16 is still an empty state. No other new action is added. Nothing approves or merges from the overview.

### 3.6 Build slot (later)
- **Shows:** "Build slot held by dev-4 (verify) · 6 min", or "Build slot free". It never says "building", because the
  slot holder may be waiting.
- **Required protocol work:** an optional `buildSlot {holder, kind, since}` on the run item. It reuses the slot state the
  daemon already holds, with no new query and no polling, and an `invalidate run` effect when the slot changes. The
  architect should confirm where that state lives. Until then, only `verify.result` outcomes are shown.

### 3.7 Coming up for you (later)
Moved from should to later. The waiting groups alone say what needs the user now. A future version needs design-package
counts on `PlanItem`. Until then, design approvals are shown only as real `design:` waiting items.

## 4. Layout

Direction A tokens throughout: the glass sidebar inset 8 px; type at 13, 15 and 20; SF Mono for anything typed; one
amber accent; flat content with hairlines. Glass only on the floating layer. Overview rows and untrusted text are never
glass.

**Nothing waiting, wide window:**

```
 payments-api                                        [toolbar capsules: glass]
 ┌ status strip ──────────────────────────────────────────────────────────┐
 │ Needs attention · 3 working · 1 stalled · 1 lost · 2 idle              │
 │ current as of 14:05                                                    │
 └────────────────────────────────────────────────────────────────────────┘
 Nothing waits for you.

 AGENTS NOW
 dev-2   developer  ✕ Lost        Ledger export (task)
 dev-4   developer  ◷ Stalled     Refund ledger entries
 dev-1   developer  ● Working 6m  Retry timeouts for webhook delivery
 pm-1    PM         ● Working     Coordinating plan-14
 View all 8 agents ›

 PLANS IN PROGRESS
 Refund ledger        ▰▰▰▱▱▱  2 unassigned · 1 reported, not integrated · 3 of 6 integrated
 Onboarding (plan-8)  ▱▱▱▱    submitted for approval

 RECENT OUTCOMES                                     last 5 · host time
 14:03          ✓ review-12 passed
 13:55          ↑ dev-1 reported a03f9c1
 yesterday 18:40 ⑂ integration 11 finished
 Show more
```

**When items wait:** the status strip stays first and the waiting groups follow, unchanged. Below them, under a
"Project" divider, Agents now shows exceptions only, and Plans and Recent outcomes fold into one expandable row each.

**Inspector:**
- Nothing selected: the project summary, the three freshness signals spelled out, and the project actions.
- An agent row: the screen 09 inspector.
- A plan row: the screen 10 inspector.
- An outcome line: the type, actor, entity, `seq`, local receipt time, and host time (labelled). Text from the event is
  shown in the untrusted block.

**640 × 480 (compact, as screen 06):**
- The sidebar folds and the inspector opens as a sheet.
- The list is one vertical scroll with no horizontal scroll.
- Agent rows stack: the id and pill on the first line, the task on the second.
- Plan fact lines wrap.
- The status strip wraps to as many lines as it needs (larger text included) and is never cut.

**Stable identity:**
- Selection, keyboard focus and scroll position are kept by item id (agent id, plan sequence, event id) when rows
  reorder or update.
- If the selected item leaves, the selection clears, the inspector shows the project summary, and VoiceOver announces
  "dev-4 is no longer listed".

**States:**
- Loading: the skeleton rows of screen 06.
- Refreshing: counts show "–" and the line says "Refreshing…". Pending items are dimmed with their last-known value.
- Refresh failed: "Refresh failed at 14:06 · showing last known from 14:02", with Retry.
- History: loading, backfilled, session only, error, truncated and gap (section 3.4).
- Empty sections:
  - Agents: "No active agent".
  - Plans: "No plan in progress".
  - Outcomes: "No outcomes since you connected at 14:02".
- Error: the screen 11 banner, with sections dimmed and marked "last known".
- Hover and focus: as on screens 01 and 06.

**Accessibility (WCAG 2.2 AA):**
- VoiceOver:
  - The status strip is one element with a value such as "Needs attention: 3 working, 1 stalled, 1 lost. Current as of
    14:05."
  - Each pill has an accessibility value (its health word).
  - The progress bar's value is "3 of 6 integrated".
  - Section titles are headings.
- Keyboard: the list is one Tab stop. The arrow keys cross sections, Return opens the inspector, and View all and Show
  more are reachable.
- Increase Contrast: hairlines become separators, and pills get outlines.
- Reduce Transparency: the glass becomes opaque window background.
- Reduce Motion: new outcome lines fade instead of sliding in (200 ms, ease-out).
- Colour is never the only signal: every pill has a word and a symbol.

## 5. Risks

**Untrusted text.**
- `task`, plan `title`, `summary`, reasons and `blocker` come from agents.
- In the list, each is one plain-text line with an ellipsis, never a link or markup, and keeps its source ("task of
  dev-4"). In the inspector, it sits in the labelled untrusted block that names its source.
- The client also isolates bidi text: it wraps each field in an isolate and replaces the remaining control and format
  characters. This is on top of the server's `clean_text`, so an agent can't reorder the line around it.
- `textVerified` false shows "Text failed its integrity check", with no stand-in.
- The status strip and the plan fact lines are built only from counts and states, never from agent text.

**Daemon hot path.**
- Every must section except backfill is client only.
- `feed recent` is bounded by rows, scan and bytes, uses a fixed head, runs per selection, and is rate-limited.
- The build slot reuses cached state.
- Note for the architect: the plan-132 agent query uses three correlated subqueries on `agent_state_history`, where one
  would do.

**Noise and alerts.**
- No new notifications.
- Backfill, reconnect and `resync` never retrigger an alert: the existing notify baseline holds, and backfill bypasses
  the reducer.
- Agents now and Recent outcomes are capped, and idle agents only add to the count.

**Stale data.** The three freshness signals are shown separately (section 2). Unknown is shown as unknown.

## 6. Answers to external-review-23

1. **Freshness: agree.** Section 2: local receipt times; the heartbeat is host-wide only; availability is a separate
   signal; Refreshing until `snapshotEnd`; the 3-interval rule; host time is never used for ages.
2. **Facts, not predictions: agree.** Section 3.3 uses facts only. Coming up for you moved to later (3.7).
3. **Bounded recent history as a must: agree.** Section 3.4: selected project only; budgets of 20 events, 500 rows and
   48 KiB; fixed head and epoch; de-duplication by project, epoch, seq and k; display-only; 3 to 5 outcomes by default.
4. **Status above the waiting groups, capped agents, compact layout, stable identity: agree.** See section 4.
5. **Protocol facts:**
   - **Agree** for `report.recorded`, `review.verdict`, the RunItem pause actor and time, Message PM, the build slot
     wording, and design counts.
   - **Partly agree** for `branch`: the v1 document lacks it, but the host code already sends it as an additive field,
     so the work is to document it, and the app treats it as optional.

**Also raised:**
- Lost is counted, and the status words separate Paused, Unknown and Quiet with a fact line (revision 3 drops Completed and No work assigned).
- Provenance and bidi handling: section 5.
- Accessibility: section 4.
- No new notifications: section 5.

**Second look (revision 3), all agreed:**
1. Receipt versus occurrence time: section 2. History rows use host occurrence time with the skew-aware age rule.
   Transport timeouts use monotonic time.
2. "Current as of" holds through invalidations: Refreshing at once (empty snapshots included), pending items, and a
   failed-refresh state (section 2).
3. State rules (section 3.1):
   - Needs you alone is not Moving.
   - Completed and No work assigned are dropped in favour of Quiet with a fact line.
   - The plans section includes submitted, in_review and signed_off-but-unmerged plans (3.3).
   - The example strip is fixed.
4. History states: loading, session only, error, truncated, no outcomes in scan, and gap with refetch (section 3.4).

Non-blocking points, both done: exceptions are capped at 4 with a count, and the strip wraps freely.

## 7. Open questions

1. `feed recent`: is it a new route, or `feed page` with a backward mode? I need the architect's view before phase 3.
2. Should the backfill persist on disk between app launches? This proposal says no: it is display-only and read again
   per connection.
3. Should the host send an authoritative merged or completed plan state, so that a "Completed" word can return?
