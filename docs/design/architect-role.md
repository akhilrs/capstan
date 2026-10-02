# Architect role, tiered planning flow and PM-owned Nexora tracking

**Status:** proposal, for the user's review. No code changes in this document's commit. Sections 0 to 9 design the Architect role and the tiered flow; section 10 adds PM-owned Nexora tracking (user decision: the PM handles all Nexora work), and its steps and questions are appended to sections 8 and 9.
**Naming:** follows `docs/design/cstan-dash-v2.md` (kebab-case file in `docs/design/`). The decisions in `decisions/` are `DEC-NNN-*.md`; if this is approved, the plan of record is a new `decisions/DEC-007-architect-role.md` that points here.
**Base:** `main` at `691ac09`. `README.md` was requested as reading but is not tracked on this branch or on `main`; the design is grounded in the code, `MVP_PLAN_V2.md` and `decisions/DEC-005-capstan-v2-direction.md` instead.

## 0. Decisions taken by the user (not re-opened here)

1. One Architect role. No separate solution-architect role.
2. The PM sets a tier at intake; the user may override it.
   - **small** (single file, docs, mechanical): PM sends straight to a developer. No architect.
   - **normal**: PM → Architect writes a plan → PM spawns developers per work package.
   - **high-risk** (schema or migrations, security or auth, public contracts or wire formats, cross-cutting): as normal, plus an independent plan review before the plan is final.
3. The plan is structured data in the ledger: work packages (owned files or areas, interfaces, dependencies and order, acceptance criteria, risks) plus the tier.
4. The Architect writes no product code and is not a second PM. It owns integration: it runs `cstan integrate` for reviewed reports, decides conflict handling (asks the PM to assign a developer), and signs off the integration branch. The user keeps the final merge to main. The Architect never merges to main.
5. Minimal added complexity and latency. Reuse existing kinds, seats, ledger records, review and integrate paths.

## 1. Summary and flow

The design adds **no new agent kind**. The Architect is a role of kind `Developer` that the configuration designates (`[architect] role = "architect"`). That reuses the seat, worktree, prompt, report-capable, supervised and replaceable machinery as is, and avoids rebuilding the `CHECK (role IN ('PM','Developer','Verifier','Supervisor'))` constraints that appear in migrations 0001, 0014 and 0015. DEC-005 item 5 states that the migration for roles is additive and that the existing role columns keep their CHECK constraints; a fifth kind would break that. Section 2 lists the cost of the alternative.

New ledger objects: `plans`, `plan_revisions`, `plan_packages`, `plan_signoffs`, and a plan subject on `reviews`. New command: `cstan plan <open|submit|show|assign|signoff>`. Changed: who may run `request-review` and `integrate`, and who gets the report and review notices.

```
small      user ─► PM ─► spawn developer ─► send task ─► report ─► request-review ─► (integrate) ─► user merges
                          (unchanged from today; no plan, no architect)

normal     user ─► PM ──plan open normal──► spawn architect ──send requirements──►  Architect reads code, writes plan
                                                                                      │ plan submit (JSON)
                                       controller: plan approved ──► message to PM ◄──┘
           PM ──plan assign <plan> <pkg> <dev>──► controller sends the package text to the developer
           developers: commit, report ──► "Verified report … Work package …" to the Architect (not the PM)
           Architect: request-review <report> ─► reviewer ─► "Review …" to the Architect
           Architect: integrate <reports…> ─► merged | conflicted(→ send @pm; PM assigns a developer to resolve)
           Architect: request-review <integration> ─► plan signoff ─► "Plan … signed off" to the PM
           PM tells the user ─► USER merges the integration branch ─► integrate confirm (PM or Architect)

high-risk  as normal, with one inserted gate:
           Architect: plan submit ─► state in_review, controller starts a plan review (reviewer role, fresh agent)
                       pass     ─► plan approved ─► PM notified
                       findings ─► plan back to draft, "Review …" to the Architect, who revises and resubmits
```

## 2. Role definition

**Kind: reuse `Developer`.** The ledger kind, the seat (`createSeat`, `Launcher.#seat`, `src/launcher.ts:472`), the worktree and branch (`capstan/<agent-id>-g<generation>`, created in `Launcher.spawn`) and the supervision (`src/supervision.ts:53` watches Developer and Verifier) are reused unchanged. The Architect gets a read-only worktree at the project HEAD so it can read the code; it commits nothing.

**Rejected alternative: a fifth kind `Architect`.** It gives per-kind capabilities and a clearer `status`, but SQLite cannot alter a CHECK, so migration 0022 would rebuild `actors`, `role_capabilities`, `seats`, `agents`, `role_definitions` and `work_items` with their triggers and foreign keys (the pattern of migration 0019 for one table), and `ROLE_KINDS`, `roles` (`src/controller/types.ts`), `AgentKind`, the dashboard and every `kind ===` branch change. That is the largest single risk in the project for a name. If a later need (for example a separate dashboard column) justifies it, the designation below is the only thing that has to be replaced.

**Designation.** `agent.roleName === config.architect.role` (`AgentRecord.roleName`, `src/controller/types.ts`). Command handlers already authorize by `caller.kind` (`src/commands.ts:313,766,990`); the new handlers authorize by kind `Developer` and the designated role name. The role is synced into the ledger with its hash (`role_definitions`, `Launcher.#assertRoleSynced`), so a config edit changes the designation only through the existing sync.

**Access per `cstan` route** (`ROUTES` in `src/daemon.ts:81`; the access column is the route-level gate, the handler adds the role check):

| Route | Architect | Change |
| --- | --- | --- |
| `inbox`, `ack`, `wait`, `status` | yes | none |
| `send` | only to the PM | none; the rule at `src/commands.ts:478` already limits non-PM agents to the PM |
| `report` | allowed but not used | none; the prompt tells it not to |
| `observe`, `finding` | no | `observe` is Supervisor/PM only (`src/commands.ts:913`) |
| `plan submit`, `plan show`, `plan signoff` | yes | new route `plan`, access `any` |
| `plan open`, `plan assign` | no | PM or operator |
| `request-review` | yes (reports, integrations, its own plan if high-risk is resubmitted by hand) | handler accepts PM or Architect (`src/commands.ts:764`) |
| `integrate <report-id>…`, `integrate discard`, `integrate confirm` | yes | handler accepts PM, operator, Architect (`src/commands.ts:831`, replacing `workerManager` for this one route) |
| `spawn`, `release`, `replace` | no | unchanged; `workerManager` stays PM or operator |
| `review` | no | Verifier only |

**May:** read the repository from its worktree, write and revise plans, request reviews, run integrations, sign off, message the PM. **May not:** edit files, commit, push, merge, spawn or release agents, message developers directly. The last rule holds because of `src/commands.ts:478`: developers and the Architect can send only to the PM, and the Architect states conflicts and package questions to the PM.

**Enforcement.** The role in `capstan.toml` uses the reviewer's tool rules (`deny = ["Write","Edit","NotebookEdit","Agent","Task","Bash(git push)","Bash(git push *)","Bash(git merge *)"]`). These are Claude Code tool rules, not a sandbox (`src/config/capstan-config.ts:232`). The hard guarantees are in the controller: `settleIntegration` refuses `confirm` unless the integration commit is already in HEAD (`src/integration.ts`, `not_in_head`), and the Architect has no route that moves HEAD. A determined agent could still run `git -C <main checkout> merge`; DEC-005's trust model (one trusted user, same VM) already accepts that for every worker.

**Prompt.** `ARCHITECT_REFERENCE` in `src/prompts.ts`, used when `PromptInput.kind === "Developer"` and `PromptInput.isArchitect`. `PromptInput` gains `isArchitect?: boolean`; the two call sites are `src/launcher.ts:767` and `:1480`. Draft:

> You are the architect of a Capstan delivery team. Your agent id is `<id>`. You plan and integrate; you never edit or commit project files and never push or merge. When the PM sends you a plan id and requirements: read the code in your worktree, then submit one plan with `cstan plan submit <plan-id> "<json>"`. Split the work into the fewest work packages that can proceed in parallel; give each package the files or areas it owns (no two packages that may run at the same time own the same file), the interfaces it must keep or add, its dependencies, testable acceptance criteria and its risks. A message from `controller` that starts with `Verified report` names the package; request a review (`cstan request-review <report-id>`), and when every package you want is reviewed, run `cstan integrate <report-id>...`, then `cstan request-review <integration-id>`. On a conflict, send the PM the report and files and ask for a developer to resolve it as a new report. When the integration review passes, run `cstan plan signoff <plan-id> <integration-id> "<summary>"`. The user merges to main; you never do.

## 3. Ledger changes

One migration, `migrations/0022_plans.sql`, additive. Tables follow the style of 0017 to 0019: STRICT, composite keys on `project_id`, immutable content enforced by triggers, state moves forward only.

```sql
CREATE TABLE plans (               -- one row per plan, mutable only in state and pointers
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  plan_id TEXT NOT NULL,           -- SAFE_AGENT_ID-compatible, controller-generated
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  title TEXT NOT NULL,
  tier TEXT NOT NULL CHECK (tier IN ('normal','high_risk')),   -- small never reaches the ledger
  state TEXT NOT NULL CHECK (state IN ('draft','in_review','approved','superseded')),
  requested_by TEXT NOT NULL,      -- PM or operator actor
  architect_agent_id TEXT,         -- set by the first submit
  current_revision INTEGER NOT NULL DEFAULT 0,
  approved_revision INTEGER,
  supersedes_plan_id TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id), UNIQUE (project_id, sequence));

CREATE TABLE plan_revisions (      -- immutable
  project_id TEXT NOT NULL, plan_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK (revision >= 1),
  base_sha TEXT NOT NULL CHECK (length(base_sha)=40),   -- HEAD when submitted
  body_json TEXT NOT NULL CHECK (json_valid(body_json)),
  body_sha TEXT NOT NULL, author_agent_id TEXT NOT NULL, author_actor_id TEXT NOT NULL, created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id, revision),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id));

CREATE TABLE plan_packages (       -- the mutable part: who works on a package
  project_id TEXT NOT NULL, plan_id TEXT NOT NULL, package_id TEXT NOT NULL,
  assignee_agent_id TEXT, assigned_at TEXT, assignment_message_id TEXT,
  PRIMARY KEY (project_id, plan_id, package_id),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id));

CREATE TABLE plan_signoffs (       -- immutable
  project_id TEXT NOT NULL, plan_id TEXT NOT NULL, integration_id TEXT NOT NULL,
  architect_agent_id TEXT NOT NULL, summary TEXT NOT NULL, created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id, integration_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id));
```

`plan_packages` rows are created when a revision is approved, one per package id in the approved body, so after approval only two kinds of column change: `assignee_agent_id`/`assigned_at`/`assignment_message_id` (by `plan assign` and `Launcher.replace`) and `cancelled_at` (by `plan cancel`, section 10). Both are guarded by the triggers below.

**Plan body** (`plan_revisions.body_json`; parsed once at the boundary by a new pure module `src/plans.ts`, "parse, don't validate"):

```json
{
  "summary": "one paragraph",
  "packages": [
    { "id": "wp1", "title": "…", "role": "developer",
      "owns": ["src/foo/", "test/foo.test.ts"],
      "interfaces": ["exports parseFoo(text): Foo"],
      "depends_on": [],
      "estimate_hours": 3,
      "acceptance": ["parseFoo rejects empty input", "npm test passes"],
      "risks": ["touches the wire format"] } ],
  "risks": ["…"],
  "integration_order": ["wp1", "wp2"]
}
```

Validation (`src/plans.ts`): 1 to `max_packages` packages; ids match `^[a-z][a-z0-9-]{0,31}$` and are unique; `depends_on` references existing ids and is acyclic; each package has a non-empty `acceptance` and an `estimate_hours` number greater than 0 and at most 80 (section 10); two packages whose `owns` overlap (equal path, or one is a directory prefix of the other) must be ordered by `depends_on`, else the plan is refused with `overlap`; `integration_order` is a permutation of the package ids that respects `depends_on` (default: topological order); body at most 32 KiB (the frame limit is `MAX_FRAME_BYTES` 64 KiB, `src/daemon.ts:40`). Text fields go through `normalizeText` (`src/text.ts`) like review text.

**States.**

| State | Meaning | Moves |
| --- | --- | --- |
| `draft` | opened by the PM, or sent back by a review with findings; the Architect is working | → `in_review` (high-risk submit), → `approved` (normal submit) |
| `in_review` | a plan review is open (`reviews.state = 'started'`) | → `approved` (pass), → `draft` (findings, failed or cancelled review) |
| `approved` | final; work packages can be assigned | → `superseded` |
| `superseded` | a later approved plan names this one in `supersedes_plan_id` | terminal |

A trigger on `plans` enforces forward moves except `in_review → draft`, as `integrations_move_forward` does in 0019. Cancellation (section 10) is a flag, not a fifth state, so the four states the user decided stay as they are. The cancellation columns are added by the Nexora migration (step 12) and carry these triggers:

```sql
ALTER TABLE plans ADD COLUMN cancelled_at TEXT;
ALTER TABLE plan_packages ADD COLUMN cancelled_at TEXT;
CREATE TRIGGER plans_cancel_once BEFORE UPDATE OF cancelled_at ON plans
  WHEN OLD.cancelled_at IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a cancelled plan stays cancelled'); END;
CREATE TRIGGER plans_frozen_after_cancel BEFORE UPDATE OF state, current_revision, approved_revision ON plans
  WHEN OLD.cancelled_at IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a cancelled plan cannot change'); END;
CREATE TRIGGER plan_packages_cancel_once BEFORE UPDATE OF cancelled_at ON plan_packages
  WHEN OLD.cancelled_at IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a cancelled package stays cancelled'); END;
CREATE TRIGGER plan_packages_frozen_after_cancel BEFORE UPDATE OF assignee_agent_id, assigned_at, assignment_message_id ON plan_packages
  WHEN OLD.cancelled_at IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a cancelled package cannot be reassigned'); END;
```

A resubmit after findings adds revision n+1; a review round is bound to one revision. An approved plan is never edited; a change is a new plan that supersedes it (`cstan plan open … <old-plan-id>`), and the old plan's already assigned packages keep running until the new plan's approval, after which assignments of still-unfinished packages must be re-created on the new plan (the PM does that with `plan assign`).

**Review of a plan: reuse the review path.** Migration 0023 (separate, because it rebuilds `reviews` exactly as 0019 did) adds `subject_plan_id TEXT` and `subject_plan_revision INTEGER`, widens `CHECK ((subject_report_id IS NOT NULL) <> (subject_integration_id IS NOT NULL))` to exactly one of three subjects, adds `one_open_review_per_plan`, and extends the author checks (`author_agent_id` is the Architect for a plan). For a plan review `commit_sha` and `base_sha` both hold the revision's `base_sha`, so the reviewer worktree (`Launcher.spawn(role, {baseSha})`, `src/reviews.ts:requestReview`) is the code the plan was written against. `reviewTask` (`src/controller/core.ts:363`) gets a plan branch that embeds the plan JSON (as quoted data, like the author summaries) and says what a plan review checks: owned areas overlap, missing interfaces, untestable acceptance criteria, missing risks, ordering. `cstan review pass|findings` is unchanged for the reviewer. `completeReview` (`src/controller/core.ts:3289`) gains one step: for a plan subject it moves the plan `in_review → approved` on pass (creating `plan_packages` rows, superseding the old plan) or `in_review → draft` on findings, in the same transaction.

**Work package ↔ assignment ↔ report.** The ledger's old `work_items`/`assignment_attempts` tables belong to the superseded fixed-seat model (`assign` is still a stub, `src/daemon.ts:ROUTES`), so v2 reports are tied to agents, not assignments. The link is:

- `plan_packages.assignee_agent_id` is set by `plan assign`.
- A report is attributed to a package by `agent_reports.agent_id = plan_packages.assignee_agent_id` (the report's generation is the agent's current one). No new column on `agent_reports`. Package progress is a derived read, not stored: `assigned` (no accepted report), `reported` (accepted report, no finished review), `findings` (latest review has findings), `reviewed` (latest review passed, not in a confirmed integration), `integrated` (in a `merged` or `confirmed` integration, from `integration_reports`).
- `Launcher.replace` (`src/launcher.ts`) moves the assignee to the new agent id in the same step as the replacement, and `buildSeed` (`src/seed.ts`) adds the package text to the replacement seed.

**Capabilities.** Migration 0022 inserts into `role_capabilities`: `plan:write` and `plan:read` for `PM`, `plan:write` (submit, signoff) and `plan:read` for `Developer`, `plan:read` for `Verifier`, and `review:request` for `Developer` and `controller`. Existing agents get grants by the same `INSERT OR IGNORE INTO capability_grants … SELECT` pattern as 0017 and 0018. The capability is per kind, so every Developer holds `plan:write` in the ledger; the handler's role-name check is the real gate. That is a weaker second layer than a distinct kind and is listed under risks.

**Notices.** Both existing notices are hard-wired to the PM (`#noticeParties`, `src/controller/core.ts:3156`; `#announceReport` at `:2921`; `#announceReview` at `:3372`). Change:

- `#announceReport`: if the reporting agent is an assignee of a package of an approved plan and the Architect of that plan is active, queue the notice to the Architect with one added line `Work package: <plan-id>/<package-id>`; else to the PM, as today.
- `#announceReview`: queue the notice to the agent that requested the review (`requested_by_actor_id`), PM or Architect.
- The PM is told only at plan milestones (section 4), not per report.

## 4. Commands and controller message prefixes

All syntax is positional (no flags), because arguments are plain strings (`args` in `src/daemon.ts`) and the wire is `MAX_ARGS` 16. One route, `plan: { access: "any" }`, with subcommands, like `integrate` and `finding check`. The CLI usage line in `src/cli.ts:820` and `runRouted`'s timeouts (`plan submit` in the high-risk case spawns a reviewer, so it joins the `LAUNCHER_CLIENT_TIMEOUT_MS` list, and `limitMs` in `src/commands.ts:1196`) change.

| Command | Caller | Effect and answer |
| --- | --- | --- |
| `cstan plan open <normal\|high-risk> "<title>" [<superseded-plan-id>]` | PM, operator | Creates a `draft` plan with the tier the PM chose. Answer: `planId`, `tier`, `state`. The tier is the PM's recorded decision; a user override is the PM running `open` with the other tier. |
| `cstan plan submit <plan-id> "<json>"` | designated Architect | Validates the body (section 3), stores revision n. Normal tier: plan → `approved`. High-risk: plan → `in_review` and the controller starts a plan review with `chooseReviewerRole(config, config.architect.reviewerRole)` (the same call `requestReview` uses). Answer: `revision`, `state`, and for high-risk `reviewId`, `reviewerAgentId`. Refused: `invalid_plan` (with the first reason), `plan_not_open` (state is not `draft`), `not_architect`. |
| `cstan plan show [<plan-id>]` | any agent, operator | Without an id: one line per plan (id, tier, state, title). With an id: the approved or current revision, each package with its assignee and derived progress, and sign-offs. Read only; output is data. |
| `cstan plan assign <plan-id> <package-id> <agent-id>` | PM, operator | Requires `approved`, a known package, an active Developer-kind agent that is not the Architect, and an unassigned package (or its assignee not active). Sets `assignee_agent_id` and queues the task to that agent in one transaction. Answer: `messageId`. |
| `cstan plan signoff <plan-id> <integration-id> "<summary>"` | designated Architect | Requires the integration in state `merged` whose reports all belong to this plan's packages, and its latest review `passed`. Inserts the sign-off and queues the PM notice. |

Messages written by the controller (the leading text is the contract, like `Verified report` and `Review`):

| Prefix | To | When |
| --- | --- | --- |
| `Plan <plan-id> approved` | PM | normal submit; high-risk review pass. Lists the packages, their dependencies and the order. |
| `Plan <plan-id> needs attention` | PM | a high-risk plan used `MAX_REVIEW_ROUNDS` (5, `src/controller/core.ts:MAX_REVIEW_ROUNDS`) without a pass, or the Architect was lost while the plan is `draft`. |
| `Work package <plan-id>/<package-id>` | developer | `plan assign`. Body: the package fields, the project rules ("commit on your own branch, never push or merge, report with `cstan report`") and the sentence that only listed `owns` areas may be changed. |
| `Verified report …` (existing) plus `Work package: <plan-id>/<package-id>` | Architect | an assigned developer's report is accepted. |
| `Review …` (existing) | Architect | verdict of a review the Architect requested, including plan reviews. |
| `Integration <integration-id> conflicted` | none new | the answer of `cstan integrate` already carries the report and files (`src/commands.ts:896`); the Architect sends them to the PM by hand. |
| `Plan <plan-id> signed off` | PM | `plan signoff`. Gives the integration branch and head commit and the sentence that the user merges it. |
| `Review <id> of plan <plan-id> revision <n>, round <r>: PASS|FINDINGS` | Architect | the existing `reviewNotice` text with the plan subject. |

## 5. Integration ownership

| Action | Today | With the Architect (plan-governed work) | Small path |
| --- | --- | --- | --- |
| `request-review <report-id>` | PM | Architect (PM still may) | PM |
| `integrate <report-id>…` | PM or operator | Architect (PM and operator still may) | PM |
| decide on a conflict | PM | Architect decides; asks the PM to assign a developer | PM |
| `request-review <integration-id>` | PM | Architect | PM |
| merge the branch into HEAD | PM, per the PM prompt | **the user**, never the PM or Architect | see Open question 1 |
| `integrate confirm` | PM | PM or Architect, after the user merged | PM |
| `integrate discard` | PM | Architect (PM may) | PM |
| sign-off | none | Architect (`plan signoff`) | none |

How the PM sees outcomes: `plan show` (progress per package), `Plan … approved`, `Plan … signed off`, `Plan … needs attention`, the existing `cstan status` (which gains a `plans` summary from `core.statusSnapshot()`), and a conflict relayed by the Architect through `send @pm`. The controller keeps no new automatic merge behaviour: conflicts are still reported, never resolved (`src/integration.ts` header comment). The one-running-integration rule (`one_running_integration` in 0019) and the review gate in `beginIntegration` (`src/controller/core.ts:3463`) are unchanged and enforce the same order for the Architect as for the PM.

## 6. Configuration (`capstan.toml`)

A new optional table, parsed in `src/config/capstan-config.ts` next to `[supervision]`, with an unknown-key check, and added to `STARTER_CONFIG` commented out so existing projects are unaffected:

```toml
[architect]
enabled = false                # off: no plan commands in prompts, behaviour is exactly today's
role = "architect"             # a role of kind Developer; must exist when enabled
plan_review = "high_risk"      # "high_risk" | "always" | "never"; the review the tier gate runs
reviewer_role = "reviewer"     # a Verifier role; default as chooseReviewerRole
max_packages = 8
count_toward_worker_limit = false
high_risk_triggers = ["schema or migrations", "security or auth", "public contracts or wire formats", "cross-cutting changes"]

[roles.architect]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)"]
prompt = "…"                   # the section 2 text is the built-in reference; this adds project rules only
```

- `high_risk_triggers` is prompt text for the PM (rendered in `PM_REFERENCE`), not a controller classifier. The tier stays the PM's judgement, as decided. The thresholds the user asked for are therefore the list of triggers plus the `plan_review` mode: `high_risk` reviews only high-risk plans, `always` reviews normal plans too, `never` disables the review gate (for a project that accepts the risk).
- Validation: when `enabled`, `role` must name a role of kind `Developer` on a `claude` host (the deny rules need a Claude host, as `rejectUnenforceable` requires); `reviewer_role`, if set, must name a `Verifier` role; `max_packages` is 1 to 20 (`MAX_INTEGRATION_REPORTS` is 20, `src/controller/core.ts`).
- `count_toward_worker_limit = false` makes `Launcher.spawn` skip the Architect when it counts active workers (`src/launcher.ts:1358`, which already excludes the Supervisor), so a waiting Architect does not take a developer slot from `[limits] max_workers`.
- The PM prompt (`PM_REFERENCE`, `src/prompts.ts`) gets the tier rules and the five plan commands only when `enabled`; it also drops the sentence "merge the integration branch into the project's HEAD yourself" for plan-governed work and says to tell the user the branch instead.

## 7. Latency and cost per tier

Costs are counted in agent sessions started, controller round trips and model waits. No timing was measured for this proposal; step 9 below records the ledger timestamps that would measure it. Spawning an agent is the slow, expensive step (Herdr worktree and pane, Claude start); everything else is a message.

| Tier | Today | With the Architect | Added | Saved |
| --- | --- | --- | --- | --- |
| **small** | PM → spawn developer → task → report → review → (integrate) | identical, and the architect is not spawned | nothing | nothing |
| **normal** | PM decomposes in its own context, spawns N developers, relays every report and runs every review and the integrate itself | `plan open` + spawn Architect + plan written (one model turn sequence) + approved notice, then `plan assign` per package; the Architect runs reviews and integrate | one agent session (Architect), one plan-writing wait, one approved notice, N `plan assign` calls (each replaces a PM-authored `send` of the task, so net zero) | PM turns per report (N report notices, N review notices and the integrate turns leave the PM's context); parallelism that a good package split enables |
| **high-risk** | as normal, no plan review | as normal plus a plan review | one reviewer session and review round (spawn + read + verdict + release, the same cost as one existing report review); up to `MAX_REVIEW_ROUNDS` repeats on findings | plan defects found before developers start rather than at integration |

Rules that keep it cheap: the Architect is spawned at `plan open` time, in parallel with the PM writing the requirements, not after; `plan submit` for the normal tier needs no extra round trip (state goes straight to `approved`); `plan assign` composes the developer task from the plan so the PM neither writes nor pastes it; report and review notices go to the Architect directly, so the PM is not a relay; the Architect stays alive across plans for the same objective (one `release` at the end), so its code reading is paid once. For single-package work the normal tier is slower than today (the extra session and plan wait) and the PM should choose small unless a package split or integration is expected; that is a stated cost, not a hidden one.

The controller work added per tier is a handful of SQLite transactions (the same cost class as `report`); the only added processes are the Architect (long-lived) and, in high-risk, one reviewer per round (short-lived, already released by `releaseReviewerLater`, `src/reviews.ts`).

## 8. Implementation plan

Fourteen steps, each one developer report (steps 1 to 9 here build the Architect role and flow; steps 10 to 14, in the second table below, add Nexora tracking): one branch, tests green with `npm run check`, no step changes behaviour while `[architect].enabled` is false. Order is dependency order; steps 1, 2 and 3 can be done in parallel by different developers because they touch disjoint files.

| # | Step | Files | Tests |
| --- | --- | --- | --- |
| 1 | `[architect]` config table: parse, validate, starter config (commented), `cstan config check` output | `src/config/capstan-config.ts`, `test/config.test.ts` | accept a valid table; reject an unknown key, a role that does not exist, a non-Developer role, a non-claude host, `max_packages` out of range; defaults leave `enabled = false` |
| 2 | Plan body parser and validator (pure) | new `src/plans.ts`, new `test/plans.test.ts` | table-driven: valid plan; duplicate id; missing acceptance; unknown dependency; dependency cycle; overlapping `owns` without order; overlapping `owns` with order; body over 32 KiB; control characters normalized; integration order not a topological order |
| 3 | Migration 0022 and the plain ledger methods `openPlan`, `submitPlan` (state moves, revisions, `plan_packages` on approval), `planRecord`, `listPlans`, `assignPackage`, `recordSignoff`, derived package progress | `migrations/0022_plans.sql`, `src/controller/core.ts`, `src/controller/types.ts` (`Capability`), `test/controller.test.ts` or a new `test/plans-core.test.ts` | forward-only state triggers (an update that skips a state aborts); revisions immutable; `plan_packages` only created on approval; double assign refused; progress derivation for each state using existing report and integration fixtures; migration applies on a ledger with data (existing migration tests pattern) |
| 4 | `plan` route: `open`, `submit` (normal tier only: approve at once), `show` | `src/daemon.ts` (`ROUTES`), `src/commands.ts`, `src/cli.ts` (usage line, timeouts), `test/commands.test.ts`, `test/cli.test.ts` | PM can open, Developer cannot; only the designated role can submit; submit on a non-draft plan refused; `show` output for a list and a plan; `forbidden` for a Verifier on `submit`; route listed in the usage line |
| 5 | Prompts and launcher wiring: `ARCHITECT_REFERENCE`, PM plan section when enabled, `isArchitect` in `PromptInput`, worker-limit exemption | `src/prompts.ts`, `src/launcher.ts`, `test/prompts.test.ts`, `test/launcher.test.ts` | prompt snapshot with and without `enabled` (disabled prompt byte-identical to today's, which proves no regression); Architect does not count toward `maxWorkers` when `count_toward_worker_limit` is false and does when true; `buildRolePrompt` stays under `MAX_PROMPT_BYTES` |
| 6 | `plan assign` and report routing: bind and send the package task in one transaction; `Launcher.replace` rebinds; seed text; `#announceReport` to the Architect; `request-review` and `integrate` accept the Architect; `#announceReview` to the requester | `src/commands.ts`, `src/launcher.ts`, `src/seed.ts`, `src/controller/core.ts`, `test/commands.test.ts`, `test/reports.test.ts`, `test/reviews.test.ts`, `test/review-commands.test.ts`, `test/integration.test.ts`, `test/seed.test.ts` | assign sends exactly one message and sets the assignee atomically; a report from an assignee notifies the Architect and names the package; a report from a non-assignee still notifies the PM; the Architect can request a review and integrate; a Developer other than the Architect cannot; review verdict reaches the requester; replace moves the assignee; with the Architect inactive the PM gets the notice (fallback) |
| 7 | High-risk plan review, data layer: migration 0023 (`reviews` rebuild with the plan subject), `checkReviewRequest`/`beginReview`/`reviewTask`/`completeReview` for a plan subject, the approve/back-to-draft transition | `migrations/0023_plan_reviews.sql`, `src/controller/core.ts`, `test/reviews.test.ts`, `test/controller.test.ts` | exactly one subject enforced; reviewer is not the plan's author; one open review per plan; pass approves and creates packages; findings returns to draft; a review of a superseded or non-`in_review` plan refused; migration preserves existing review rows (copy test as for 0019) |
| 8 | High-risk plan review, command layer: `plan submit` starts the review for high-risk (or `plan_review = "always"`), `Plan … approved` and `needs attention` notices, `cstan request-review <plan-id>` accepted for the Architect | `src/commands.ts`, `src/reviews.ts`, `src/controller/core.ts`, `test/commands.test.ts`, `test/review-commands.test.ts` | with stub launcher (`test/launcher-stubs.ts`): high-risk submit spawns one reviewer and sets `in_review`; reviewer failure to spawn leaves the plan `draft` and tells the Architect; findings then revised resubmit starts round 2; round 6 refused with `needs attention` to the PM; normal tier never spawns a reviewer unless `plan_review = "always"` |
| 9 | Sign-off, status and restart: `plan signoff`, `Plan … signed off` notice, `plans` in `statusSnapshot`, plans in `PmRestartSummary`, timestamps for latency measurement, end-to-end scenario per tier | `src/commands.ts`, `src/controller/core.ts`, `src/prompts.ts` (summary block), `test/plan-flow.test.ts` (new), `test/prompts.test.ts` | sign-off refused for an integration with a report outside the plan, with a non-passed review, or not `merged`; end-to-end with stubs: small (no plan, no architect), normal (open → submit → assign → report → review → integrate → integration review → signoff → user-merge simulated by `isInHead` stub → confirm), high-risk (adds the plan review with one findings round); PM restart summary lists open plans |

Nexora steps (section 10). They depend on steps 3 and 4 (the `plans` and `plan_packages` tables and the `plan` route) and are otherwise independent of the review steps 7 and 8. **Migration numbers are assigned in merge order**: the labels 0023 (plan reviews, step 7) and 0024 (Nexora, step 12) in this document are names, not reservations. The two files do not reference each other; the only coupling is that `plan cancel` of an `in_review` plan (which cancels a plan review) is meaningful only after step 7, so step 13 implements that row of the cancel table behind a check on the plan's state, and before step 7 lands no plan can be `in_review`, so the row is unreachable and untested until then; step 8's tests add the cancel-while-reviewing case.

| # | Step | Files | Tests |
| --- | --- | --- | --- |
| 10 | **Verify the Nexora assumptions first** (section 10, "Assumptions to verify"): with the Nexora tools, create a throwaway parent and child item in the project, set each status in the mapping, set `estimated_hours`, log time with `duration_minutes`, read the item back, then delete or close the items. Record the results in the step's report and amend section 10 if any assumption is false. No repository change except this document. | `docs/design/architect-role.md` | the report lists, for each assumption, the call made and the observed result; the downstream steps are not started until it is accepted |
| 11 | `[nexora]` config table (policy only): parse, validate, starter config (commented), `cstan config check` | `src/config/capstan-config.ts`, `test/config.test.ts` | defaults (`track = "never"`); reject unknown keys, a bad `track`/`default_action`; `track = "always"` with `default_action = "none"` refused as contradictory |
| 12 | Migration 0024: `external_links` (with the `synced_state` CHECK and `bound_agent_id`), `plans.cancelled_at`, `plan_packages.cancelled_at`; ledger methods `linkExternal`, `bindRequirement`, `cancelPlan`, `externalLinks`, `wantedNexoraState`, `syncDrift`. `wantedNexoraState` is a pure function of (progress, cancelled, integration confirmed) and is tested from the table in section 10, one test row per table row | `migrations/0024_external_links.sql`, `src/controller/core.ts`, `test/plans-core.test.ts` | every row of the progress table in section 10 (assigned, reported, findings, reviewed, integrated, confirmed, cancelled) yields the stated wanted status; parent wanted state for: no packages started, mixed, all reviewed, signed off, all confirmed, any cancelled and the rest confirmed, all cancelled; requirement wanted state for an unbound link, a bound agent with no report, an accepted report, a passed review, a merged integration, a confirmed integration; `external_id` immutable; `synced_state` outside the CHECK list refused; an `UPDATE` of `external_id` aborts at the trigger (tested with raw SQL, not through the method); `plan cancel` in each plan state of the cancel table, including that a later `assignee_agent_id` update on a cancelled package aborts; the rebinding query used by `replace` moves an assigned package but leaves a cancelled package of the same agent untouched and does not abort; `plan open … <cancelled-plan-id>` is refused with `plan_cancelled` and leaves the old plan unchanged; cancel-after-open: open plan B superseding approved plan A, cancel A, approve B, and B is `approved` with its packages, A is still cancelled and not `superseded`, and the PM's approved notice carries the extra line; cancel one package and then the whole plan, which succeeds, sets `cancelled_at` only on the packages not yet cancelled and not confirmed, and does not fire `plan_packages_cancel_once`; a second `plan cancel` of the same plan answers `already_cancelled`; `plan cancel` of one package that is `confirmed` answers `package_confirmed`; `plan assign` of a cancelled package answers `package_cancelled` (not a trigger abort); a review that passes after the plan was cancelled finishes the review and leaves the plan unchanged; `plan open … <approved-plan-id>` still works; the parent and requirement tables as written (rows evaluated in order, including the all-cancelled parent and the several-reports requirement); drift is empty when `synced_state` equals wanted |
| 13 | `cstan link` (`link`, `link bind`) and `plan cancel`; `plan show` Nexora columns; `Nexora drift` section in `cstan status` for the PM; the PM notice `Plan <id> package <pkg> reviewed` | `src/daemon.ts` (`ROUTES`), `src/commands.ts`, `src/cli.ts`, `src/controller/core.ts` (notice), `test/commands.test.ts`, `test/reviews.test.ts` | `link` is `access: "any"` in `ROUTES` and the handler accepts only PM or operator (same check as `plan open`), so a Developer or the Architect gets `forbidden`; `plan cancel` accepts only the operator; `Launcher.replace` of a developer whose package is cancelled succeeds, the cancelled package keeps the old agent id and the replacement seed omits it (stub launcher, `test/launcher-stubs.ts`); `plan show` prints id, synced state, wanted state and drift; the notice is queued once when the latest review of a package's report passes and `track` is not `never`, and not again for a repeat |
| 14 | PM prompt: intake picker, mirroring rules, failure rules, the "ask the user before `integrate confirm`" rule (only when `[nexora].track` is not `never`); PM restart summary lists links and drift | `src/prompts.ts`, `src/controller/core.ts` (`PmRestartSummary`), `test/prompts.test.ts` | prompt byte-identical to step 5 output when `track = "never"`; with `ask`, the prompt names the picker options and the status mapping; restart summary lists a drifted item |

Possible later steps, not part of this proposal: a dashboard view of plans (`src/dash/model.ts`), and a plan-to-integration link column if derivation proves slow.

## 9. Open questions and risks

**Open questions (for the user; none re-opens a decision above):**

1. **PM prompt versus "user merges".** The PM prompt today tells the PM to merge the integration branch into HEAD itself (`PM_REFERENCE`, the `integrate` bullet), and `integrate confirm` needs that merge first. Decision 4 says the user does the final merge. I propose to change the prompt to "tell the user the branch" for plan-governed work only. Should the small path change too, so that no agent ever merges to HEAD?
2. **Who runs `integrate confirm`.** It must come after the user's merge, which the Architect cannot observe except by trying. Proposed: the PM runs it when the user says they merged, and the Architect may too. Alternatively the controller could confirm on its own once the integration head is in HEAD (a poll in the existing driver), which saves a hop but adds automatic state change.
3. **Does a normal-tier integration still need a review?** `settleIntegration` requires a passed integration review for every `confirm` (`src/controller/core.ts:3703`), which is existing behaviour and is kept. If the user wants the normal tier faster, relaxing it is a separate, explicit change.
4. **Architect lifetime.** Proposed: one Architect for the life of an objective, released by the PM at the end. A context-heavy Architect gets slow; `replace` with the seed is the escape. Is that acceptable or should it be one per plan?
5. **Developers talk to the Architect.** Today non-PM agents can message only the PM (`src/commands.ts:478`). A developer with a question about its package goes through the PM, an extra hop. Allowing Developer → Architect messages is a small change to that rule; I did not propose it to keep the PM as the single human-facing point, but it may be worth it.

6. **Nexora (section 10).** (a) Is `estimate_hours` the right unit, given that agents work in minutes and the estimate is a planning figure? (b) Should the PM log agent wall-clock as Nexora time at all (default proposed: no)? (c) May `track = "always"` be the project default for this repository, with the picker kept for exceptions only? (d) A user who rejects a merge after `in_review`: the PM reopens the item to `in_progress`; confirm that is wanted. (e) A wrong external id cannot be edited (the id is immutable); should the PM be able to unlink and relink, or is it enough that the PM tells the user?

**Risks:**

- **Designation by role name, not kind.** Capabilities are per kind, so every Developer holds `plan:write` and `review:request` in the ledger and the handlers' role check is the real gate. A bug in one handler is an escalation path. Mitigation: one shared helper `isArchitect(agent, config)` used by all five handlers, with a test that a non-Architect Developer is refused on each. The alternative (a fifth kind) is described in section 2 and costs a large table rebuild.
- **Migration 0023 rebuilds `reviews`.** It has the same shape of risk as 0019 (immutability triggers, foreign keys, partial unique indexes must all be recreated). Mitigation: a copy-preserves-rows test and keeping it a migration of its own (step 7).
- **Soft enforcement of "never merges".** Tool rules are advisory; the guarantee is that no route moves HEAD and `confirm` checks `isInHead`. A worker with `git -C` can still merge into the main checkout, as any worker can today (DEC-005 trust model).
- **Plan quality is unchecked in the normal tier.** There is no review, by decision. The structured validation in step 2 (overlap, cycles, acceptance present) catches only structural defects. If the first real runs show weak plans, `plan_review = "always"` is the lever.
- **Latency for small-but-plural work.** Two-package work that is really sequential pays the Architect cost for no parallelism gain. The PM prompt must say so; there is no mechanical tier classification, as decided.
- **Notice routing when the Architect is lost.** The fallback to the PM (step 6) prevents a stuck queue, but the PM then holds reports it did not expect. The existing `Agent … lost` notice (`#recordLost`) already tells the PM, and `replace` rebinds packages.
- **Unverified.** I did not run the code or any timing. The claims about function locations come from reading `src/` at `691ac09`; the line numbers will move. `README.md` could not be read because it is not in the repository at that commit.

## 10. Nexora integration (PM-owned)

User decision: the PM handles all Nexora work. Workers and the Architect never call Nexora; they report through `cstan` and the PM mirrors outcomes. The controller never calls Nexora either.

### What exists today

Capstan's source does not read `.nexora.toml` or call Nexora. The only trace is the `NEXORA_API_KEY` example under `[env] pass` in `src/config/capstan-config.ts` (`STARTER_CONFIG`), which copies the variable into agents' environments. In the repository root, `.nexora.toml` holds the API URL, organization id and project code (`PRJ-019`); `capstan.toml` passes `NEXORA_API_KEY`. The PM's Claude session reaches Nexora through its own Nexora tools (the MCP tools that read `.nexora.toml`), and `PM_DEFAULT_DENY` (`src/config/capstan-config.ts`) denies only file edits and subagents, so those tools are available to the PM and nothing in Capstan needs to change to allow them. Whether they are allowed for a non-PM role is a role `allow`/`deny` matter; workers get no Nexora rule, and the prompt tells them not to use any.

### Decision: PM prompt plus a small ledger link table, no controller sync

Three options were weighed.

| Option | Reliability | Cost |
| --- | --- | --- |
| A. Prompt only; the PM remembers ids and what it synced | Weak: ids and "what is synced" live in the PM's context; a PM restart (`pm-restart`, `PmRestartSummary`) or a missed message loses them, and duplicates are created on retry | none |
| **B. Prompt + `external_links` table + drift view (proposed)** | Good: the ledger holds ids and the last synced state; the PM reconciles from `plan show` and the `Nexora drift` section whenever it wakes, so a missed message costs lag, not correctness; the controller adds one small notice (package reviewed) for the event that otherwise has none; writes stay idempotent | one table, one command, one status section |
| C. Controller sync queue with a Nexora client | Best on paper; automatic retry | The daemon needs the API key and network, an HTTP client, a mapping of statuses, error handling and a retry loop; it breaks the rule that the PM is the only Nexora writer and adds a failure surface to the process that must never block delivery |

B is the smallest option that stays reliable. Delivery never depends on Nexora, because the table is passive: nothing in the controller reads it to decide a transition.

### Policy: ask, default and the picker

A new optional table in `capstan.toml`, policy only (the connection details stay in `.nexora.toml`, which Capstan still does not read):

```toml
[nexora]
track = "ask"             # "ask" | "always" | "never"; default "never" so existing projects are unchanged
default_action = "create" # what "always" does, and the recommended picker option: "create" | "link" | "none"
```

At intake, when `track = "ask"`, the PM asks once with Claude Code's `AskUserQuestion` (the PM prompt already requires it for choices, `PM_REFERENCE`), three options, the default first with "(Recommended)": **Create a new Nexora item**, **Link to an existing item** (the PM then asks for the id as plain text, an open-ended value), **Do not track**. `always` skips the question and applies `default_action`; `never` removes every Nexora instruction from the PM prompt, so the prompt is byte-identical to today's. A user can still say "do not track this one" under `always`; the PM then treats it as `none` for that requirement.

### Mapping

| Capstan | Nexora | Where the id is stored |
| --- | --- | --- |
| requirement (what the user shared) | parent work item (type `story` or `feature`, PM's choice) | `external_links` kind `plan` for tiers normal/high-risk; kind `requirement` for the small tier (ref id chosen by the PM, such as `req-1`) |
| Architect work package | child work item (`parent_display_id` = the parent), title and acceptance criteria from the package, `estimated_hours` from the plan | `external_links` kind `package`, ref `<plan-id>/<package-id>` |
| small tier | the parent item only, no children | kind `requirement` |

```sql
CREATE TABLE external_links (          -- migration 0024
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  ref_kind TEXT NOT NULL CHECK (ref_kind IN ('requirement','plan','package')),
  ref_id TEXT NOT NULL,
  system TEXT NOT NULL CHECK (system IN ('nexora')),
  external_id TEXT NOT NULL,           -- the display id, for example PRJ-019-42; immutable once set
  synced_state TEXT NOT NULL CHECK (synced_state IN ('backlog','todo','in_progress','in_review','completed','wont_do')),  -- last status the PM wrote
  bound_agent_id TEXT,                 -- kind requirement only: the developer whose reports drive its wanted state; set by `link bind`
  linked_by TEXT NOT NULL, linked_at TEXT NOT NULL, synced_at TEXT NOT NULL,
  PRIMARY KEY (project_id, ref_kind, ref_id, system));

CREATE TRIGGER external_links_identity BEFORE UPDATE OF project_id, ref_kind, ref_id, system, external_id, linked_by, linked_at ON external_links
  BEGIN SELECT RAISE(ABORT, 'external link identity is immutable'); END;
CREATE TRIGGER external_links_no_delete BEFORE DELETE ON external_links
  BEGIN SELECT RAISE(ABORT, 'external links are not deleted'); END;
```

`external_id` immutability is enforced by the trigger, not only by the ledger method; the method `linkExternal` checks first and answers `link_conflict` so the PM gets a readable error instead of an abort. Only `synced_state`, `synced_at` and `bound_agent_id` can be updated. A wrong id is corrected by linking the item under a new ref, which a deleted-never table allows only for refs not yet linked; the PM tells the user instead (open question 6e).

A link to an item that already existed (the "link" option) uses the same row. A package is a link only after the PM creates its child item; the PM creates all child items when the plan is approved, in one pass.

### Commands

`cstan link <requirement|plan|package> <ref-id> <external-id> [<synced-state>]` inserts the row, or updates `synced_state` and `synced_at` when the row exists with the same `external_id`; a different `external_id` for an existing row is refused (`link_conflict`); a state outside the CHECK list is refused. `cstan link bind <requirement-ref-id> <agent-id>` sets `bound_agent_id` for a requirement link (the small tier); `Launcher.replace` moves it to the replacement agent, as it does for package assignees. The PM runs both after each successful Nexora write, so the ledger records only what really reached Nexora.

Authorization: `link` is one route with `access: "any"` in `ROUTES` (`src/daemon.ts`), like `plan` and `integrate`. The handler resolves the caller with `workerManager(call.identity)` (`src/commands.ts`, the PM-or-operator check used by `spawn`, `release` and `replace`) and answers `forbidden` for anyone else, which is also the check `plan open` and `plan assign` use.

`cstan plan cancel <plan-id> [<package-id>]` is new and **operator-only** (the handler accepts only the operator identity, the same rule `ROUTES` gives `cancel`, `src/daemon.ts:96`). The existing `cstan cancel <message-id>` cancels a queued message, not work, so it cannot carry this; there is no work cancellation in the controller today. It sets `cancelled_at` on the plan, or on one package; the controller then queues the PM notice `Plan <id> cancelled` (or `… package <pkg> cancelled`). The user tells the PM or runs the command themselves; the PM reacts and mirrors, it never cancels.

`cstan plan show <plan-id>` gains, per package, the external id, `synced_state`, the `wanted` state and the drift flag. `cstan status` for the PM gains a `Nexora drift` section listing linked items whose `synced_state` differs from `wanted`. This is the pending-sync record: drift is derived, so it cannot be forgotten or go stale.

### Estimates and time

The Architect's plan carries `estimate_hours` per package (a number greater than 0 and at most 80; the validator in section 3 enforces it). The unit is hours of focused agent-plus-review effort for one package, an estimate for planning, not a promise. The unit matches Nexora's `estimated_hours` field on `nexora_work_item_create`, so the PM copies it unchanged and the parent item's estimate is the sum. A re-plan produces new packages and new estimates; the PM updates the items.

Actual time is not tracked by default. The ledger can show a package's wall-clock span (`plan_packages.assigned_at` to the accepted report and review pass timestamps), and if the user wants it, the PM may log that span as Nexora time with the time-log tool (`duration_minutes`), labelled "agent wall-clock". It is not effort, and several agents run in parallel, so it is off unless the user asks (open question 6b).

### Meaning of `in_review`

In this design `in_review` means **waiting on a human**. An automated reviewer's pass does not complete anything: after it, the work still needs the user to merge to main. So a package is `in_review` from the moment its review passes until the user's merge is confirmed, and the parent is `in_review` from sign-off until then. `in_progress` is work an agent is doing (including fixing findings); `completed` is only reached through the user's merge.

### The wanted state (derived, never stored)

`wantedNexoraState(ref)` is a pure function over ledger facts. `synced_state` is what the PM last wrote; drift is `synced_state != wanted`. Package progress is the derivation of section 3; "confirmed" is added by this section.

**Packages (`ref_kind = package`):**

| Progress state | Ledger fact | Wanted Nexora status |
| --- | --- | --- |
| (not assigned) | approved plan, `assignee_agent_id` null | `todo` |
| `assigned` | assignee set, no accepted report | `in_progress` |
| `reported` | accepted report, no finished review | `in_progress` |
| `findings` | latest finished review of the report has findings | `in_progress` (the developer is fixing) |
| `reviewed` | latest review passed, report not in a `merged` integration | `in_review` |
| `integrated` | report is in a `merged` integration (not yet confirmed) | `in_review` |
| `confirmed` | report is in a `confirmed` integration | `completed` |
| `cancelled` | `plan_packages.cancelled_at` or the plan's `cancelled_at` set, and not confirmed | `wont_do` |

Precedence, first match wins: `confirmed`, then `cancelled`, then the other rows from the bottom of the table upward (`integrated`, `reviewed`, `findings`, `reported`, `assigned`, not assigned). Finished work is not unwound by a later cancel. An integration that is `conflicted`, `discarded` or `failed` leaves the package at `reviewed`: only a `merged` integration moves it to `integrated`, and only a `confirmed` one to `confirmed`. Progress is computed from the assignee's **latest accepted report** (highest `agent_reports.sequence`), so a new report after findings starts again at `reported`. Reports of an agent the assignee replaced are not counted: after `Launcher.replace` the package shows `assigned` (wanted `in_progress`) until the replacement reports. That can move an item back from `in_review`; it is accepted because the replacement has to redo the report anyway.

**Parent plan item (`ref_kind = plan`):** let *live* be the packages whose package progress is not `cancelled` (a `confirmed` package is live). Rows are evaluated top to bottom and **the first match wins**, so no row can match together with an earlier one.

| # | Condition | Wanted |
| --- | --- | --- |
| 1 | the plan has `cancelled_at` set and not every package is `confirmed`; or the plan is approved and *live* is empty (every package cancelled) | `wont_do` |
| 2 | *live* is non-empty and every live package is `confirmed` (this includes a plan cancelled after everything was confirmed, and a per-package cancel of the rest) | `completed` |
| 3 | the plan is `draft` or `in_review` (no packages exist yet), or no live package is assigned | `todo` |
| 4 | *live* is non-empty and every live package is `reviewed`, `integrated` or `confirmed` (not all `confirmed`, by row 2) | `in_review` |
| 5 | anything else (at least one live package assigned and at least one not yet `reviewed`) | `in_progress` |

A sign-off does not change the parent's wanted state on its own: after the last live package is `reviewed` the parent is already `in_review` (row 4); the sign-off is a comment trigger, not a state. A `superseded` plan keeps the wanted state it had when it was superseded and is not drift-checked afterwards.

**Requirements of the small tier (`ref_kind = requirement`):** there are no packages. The requirement is driven by the one developer bound with `link bind` (the PM binds the agent it spawns). Only that agent's **latest accepted report** R (highest `agent_reports.sequence`) counts; earlier reports of the same agent are ignored, so a fix report after findings restarts the rows. The facts are R, the latest finished review of R, and the integrations containing R. Rows are evaluated top to bottom, first match wins:

| # | Condition | Wanted |
| --- | --- | --- |
| 1 | no bound agent | none: no wanted state, no drift; the status line says "unbound" and the PM drives the link by hand with `cstan link` |
| 2 | the PM recorded `wont_do` on the link (cancellation, below) and R is not in a `confirmed` integration | `wont_do` |
| 3 | R is in a `confirmed` integration | `completed` |
| 4 | the latest review of R passed (R may be in a `merged` integration or none) | `in_review` |
| 5 | anything else: no accepted report, report without a finished review, or latest review has findings | `in_progress` |

Row 2 reads `synced_state`: a requirement has no ledger fact for cancellation, so the PM's recorded `wont_do` is itself the input. It is therefore never drift (wanted equals synced).

If the small tier skips integration (the PM does not run `cstan integrate`), the requirement stays `in_review` after the passing review until the user says the change is merged; the PM then records `completed` by hand. This is the one place the human closes the gate without a controller fact, and the PM prompt says so.

### How the user's merge is recorded

The user merges the integration branch into main with their own git. The controller can verify that: `settleIntegration` refuses `confirm` unless the integration commit is already in HEAD (`isInHead`, `src/integration.ts`). So the recorded confirmation is the **`integrations.state = 'confirmed'` row** (with `completed_at`), written when `cstan integrate confirm <integration-id>` succeeds. The PM prompt rule: after the sign-off notice the PM tells the user the branch, asks (picker: merged / not yet) and runs `integrate confirm` only after the user says it is merged; the controller then checks that the merge really is in HEAD. Before that row exists, a `merged` integration yields wanted `in_review`; after it, wanted becomes `completed` and drift appears for the PM to fix. The human act is the user's merge plus their answer; the controller supplies the proof.

### Status sync (PM actions)

| Trigger the PM sees | Nexora action by the PM | `cstan link` state written |
| --- | --- | --- |
| `Plan <id> approved` | create child items with estimates; comment on the parent: plan approved, packages and order | `todo` |
| the PM's own `plan assign` succeeded | transition the package item | `in_progress` |
| `Plan <id> package <pkg> reviewed` (new notice, below) | transition the package item; comment: reviewed, commit | `in_review` |
| `Plan <id> signed off` | comment on the parent with the integration branch and head | `in_review` |
| the user confirms the merge and `integrate confirm` is accepted | package items and the parent to `completed` | `completed` |
| `Plan <id> cancelled` / `… package <pkg> cancelled` (operator ran `cstan plan cancel`) | transition to `wont_do` with a comment | `wont_do` |
| a drift line in `cstan status` or `plan show` at any wake-up | write the wanted state | the wanted state |

**Cancellation: one rule.** The user or operator decides; the PM never decides and only records the decision. For work that has a plan, the operator runs `cstan plan cancel` (operator-only) and the ledger holds the decision; the PM sees the `Plan … cancelled` notice and mirrors `wont_do`. For a small-tier requirement there is no plan, so the user tells the PM in the conversation and the PM records `wont_do` on the link (`cstan link requirement <ref-id> <external-id> wont_do`) and releases the developer; the requirement table's row 2 treats that record as the decision. In both cases the Nexora comment names who decided ("cancelled by the user").

**What `plan cancel` does**, in one transaction, by the plan's state:

| Plan state | Effect |
| --- | --- |
| `draft` | sets `cancelled_at`; no packages exist; queues a message to the Architect (`Plan <id> cancelled`) so it stops |
| `in_review` | the open plan review is cancelled (`reviews.state = 'cancelled'`, `failure_reason = 'plan cancelled'`, which the existing CHECK for cancelled rows requires; the reviewer is released with the existing `releaseReviewerLater`), the plan moves `in_review → draft` (an allowed move), then `cancelled_at` is set; Architect and PM are notified |
| `approved` | sets `cancelled_at` on the plan and on every package that is not yet cancelled and not `confirmed` (`UPDATE plan_packages SET cancelled_at = <now> WHERE plan_id = <id> AND cancelled_at IS NULL AND <package is not confirmed>`, so a package cancelled earlier is not touched and `plan_packages_cancel_once` cannot fire); assigned developers are not released by the controller (the PM releases them, as for any worker); an `integrate` already running finishes, and the Architect is told to `discard` an unconfirmed integration |
| `superseded` | refused (`plan_superseded`) |

With a package id, only that package is cancelled (plan `approved` only). A cancelled plan refuses `submit`, `assign`, and review completion.

**Guards for every statement that touches a row the triggers freeze.** Each handler or ledger method checks the guard first and answers a named error, so a trigger abort is never the normal way a request fails. The complete list of such statements:

| Statement | Guard (checked before the `UPDATE`) | Answer when the guard fails |
| --- | --- | --- |
| `plan cancel <plan-id>` sets `plans.cancelled_at` | `cancelled_at IS NULL` and state is not `superseded` | already cancelled: no-op, answer `already_cancelled`; superseded: `plan_superseded` |
| `plan cancel <plan-id>` sets `plan_packages.cancelled_at` | `cancelled_at IS NULL` and the package is not `confirmed` (in the `WHERE`, as above) | rows that fail are skipped, not errors |
| `plan cancel <plan-id> <package-id>` | plan not cancelled; package exists, `cancelled_at IS NULL`, not `confirmed` | `plan_cancelled`, `plan_not_approved` (plan is `draft`, `in_review` or `superseded`), `already_cancelled`, `package_confirmed` |
| `plan assign` sets the assignee | plan not cancelled and package `cancelled_at IS NULL` | `plan_cancelled`, `package_cancelled` |
| `plan submit` sets `current_revision` and `state` | plan `cancelled_at IS NULL` and state `draft` | `plan_cancelled`, `plan_not_open` |
| review completion sets `plans.state` and `approved_revision` | plan `cancelled_at IS NULL` | the review row is still finished (`passed` or `findings`); the plan is left unchanged and the PM and Architect are told the plan was cancelled |
| approval sets the old plan `approved → superseded` | old plan `cancelled_at IS NULL` | the supersede step is skipped (below) |
| `Launcher.replace` moves the assignee | `cancelled_at IS NULL` in the `WHERE` | cancelled rows are skipped |
| `link`, `link bind` on `external_links` | no freeze triggers except identity columns; `external_id` is compared first | `link_conflict` |

**Cancel after a new plan was opened.** If the operator cancels the old approved plan after `plan open … <old-plan-id>` created the new plan and before the new plan is approved, the approval transaction of the new plan **skips the supersede step** when the old plan is cancelled: the new plan is approved normally (`draft`/`in_review → approved`, packages created), the old plan stays cancelled and is not marked `superseded`, and the controller queues one extra line in the `Plan <new-id> approved` notice to the PM: `Plan <old-id> was cancelled before this plan was approved; it was not superseded`. `supersedes_plan_id` on the new plan still names the old plan as history.

**Interaction with `Launcher.replace`.** The freeze trigger on `plan_packages` would abort a replace that tries to move a cancelled package to the replacement agent. The rebinding step of `replace` (step 6) therefore updates only packages with `cancelled_at IS NULL` (`UPDATE plan_packages SET assignee_agent_id = <new> … WHERE assignee_agent_id = <old> AND cancelled_at IS NULL`); cancelled packages keep the old agent id as history, and nobody works on them. The developer assigned to a cancelled package stays releasable and replaceable: `replace` and `release` never touch cancelled rows, and the replacement is not given that package (its seed omits cancelled packages).

**Interaction with supersede.** `plans_frozen_after_cancel` also blocks the `approved → superseded` move on a cancelled plan, so `cstan plan open <tier> "<title>" <superseded-plan-id>` refuses a cancelled plan with `plan_cancelled: <id> was cancelled; open a fresh plan without naming it` (and a plan that is not `approved` with `plan_not_supersedable`). Work that is cancelled is never "replaced"; a new plan is simply opened. A review that finishes after the cancel is handled by the "review completion" row of the guard table above (the review row is still finished, the plan is left unchanged, the PM and Architect are told).

### Sync lag and the one new notice

Today the controller tells the PM only about reports (`Verified report`), review verdicts (`Review …`, section 4 routes them to the Architect for plan-governed work), and delivery or agent problems; it does not tell the PM about everything. Of the events that change a wanted state, the one with no PM-visible signal is **a package becoming `reviewed`**: the review notice goes to the Architect, and the PM only wakes on notices. This design adds the smallest fix: when the latest review of a package's report passes, the controller queues one short message to the PM, `Plan <plan-id> package <package-id> reviewed` (commit sha and report id, nothing else), once per report, only when `[nexora].track` is not `never` (it is queued with the existing once-per-episode machinery, `#queuePmNotice`, `src/controller/core.ts`). The other events already reach the PM: `Plan … approved`, `Plan … signed off`, `Plan … cancelled`, and the PM's own actions (assign, confirm).

Two events are accepted as lagging, and the PM catches them at its next wake-up by running `cstan plan show` / reading the `Nexora drift` section: a `findings` verdict (the wanted status stays `in_progress`, so only the optional Nexora comment about the findings lags) and an integration result of `merged`, `conflicted` or `failed` before sign-off (the Architect sends conflicts to the PM by hand; the comment on the parent lags until sign-off). Nothing else is claimed: the PM prompt says to run `cstan plan show` after every `Plan …` notice and at every wake-up, and "the PM mirrors drift; it does not remember events".

### Assumptions to verify

These come from reading the Nexora tool schemas and were **not exercised**; step 10 checks each against the live tools before any implementation, and this section is amended with the result.

| Assumption | Used for | Source and gap |
| --- | --- | --- |
| a status `wont_do` exists and is accepted by `nexora_work_item_transition` | cancellation mirror | the tool's status enum lists `backlog, todo, in_progress, in_review, completed, wont_do`; no call was made |
| `parent_display_id` links a child to its parent on create | requirement to package mapping | present in the create schema; whether it works across item types (story, task) was not tried |
| `estimated_hours` is accepted as a decimal and shown on the item | estimates | present in the create schema; rounding and a maximum were not checked |
| time can be logged with `duration_minutes` (integer minutes, at least 1) | optional actual time | present in the time-log schema; whether the log is allowed on an `in_review` item was not tried |
| the display id format returned by create (for example `PRJ-019-42`) is accepted back by `display_id` | `external_id` | the example in the schema is `PM-42`; the exact shape for this project was not seen |
| the parent item keeps its own status independently of its children | parent wanted state | not known whether Nexora derives parent status from children |

### Failure behaviour

- Nexora unreachable, the key missing, or `[nexora].track = "never"`: delivery continues. The PM never waits on a Nexora call before a `cstan` command.
- A failed write is not recorded with `cstan link`, so the item shows as drift. The PM tells the user once ("Nexora unreachable, N items out of sync") and retries at its next wake-up, with a limit of three tries per item per session; after that it leaves the drift and says so in its next report to the user.
- A PM restart loses nothing: the links and drift are in the ledger and in the restart summary (`PmRestartSummary`, rendered in `src/prompts.ts`).
- The Nexora calls themselves are idempotent when the PM checks `plan show` first: it creates an item only when the package has no link, and transitions only when `synced_state` differs from `wanted`.
- The Supervisor and the controller do not look at Nexora; a PM that stops mirroring is visible as drift in `status` and on the dashboard in a later step.

### PM prompt additions (only when `[nexora].track` is not `never`)

Rendered in `PM_REFERENCE`: the picker rule above; "you are the only agent that writes to Nexora; never ask a worker to"; the mapping table; the status table; "run `cstan link` after every successful Nexora write"; "before any Nexora write run `cstan plan show` and write only what differs"; "Nexora failures never block delivery; tell the user once". Worker, Architect and Verifier prompts gain one sentence: "Do not use Nexora tools; the project manager records progress there."

### What this section does not change

No controller code reads or calls Nexora. No ledger transition depends on a link. The existing flows (small, normal, high-risk) are unchanged when `track = "never"`. Worker permissions: a role that lists Nexora tools in `allow` could still call them (tool rules are advisory, section 2); the guard is the prompt and a `deny` entry for the Nexora tool names in the Developer, Verifier and Architect roles, which the starter config for a tracked project should include.
