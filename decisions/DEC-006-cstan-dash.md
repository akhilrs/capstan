# DEC-006: `cstan dash`, a full-screen live terminal dashboard (Ink)

**Task:** design plan and implementation.
**Status:** accepted by the user 2026-10-02, pending operator merge of the PR that adds it. Independent plan review: PASS.
**Decision:** Add `cstan dash`: an Ink (React for terminals) full-screen view of one Capstan run. It reads the operator `status` route of the running daemon over the control socket, redraws on change, and offers three safe actions (observe an agent's screen, retry/skip/cancel a stuck message), each behind a confirm key and each routed through an existing daemon command. `cstan status` is not changed.

## Context

`cstan status` is plain text/JSON that agents and scripts parse (`CstanStatusJsonV1`, `src/cli.ts`). `cstan status --watch` (`src/watch.ts`) already polls the daemon and prints a compact block, but it scrolls, shows no pipeline, no health reason and no screens, and cannot act. The operator wants an htop/btop-style view. Decisions already taken by the user: command `cstan dash`; Ink; v1 = live read view plus a few safe, confirmed actions; `cstan status` stays untouched.

Facts checked in the code (all paths relative to the repo root):

- The daemon is the only writer of `controller.sqlite` and holds the project lock (`ControllerCore.open`, `src/controller/ownership.ts`). The DB runs in WAL mode (`openDatabase`, `src/controller/database.ts`).
- The operator branch of the daemon `status` handler (`src/commands.ts`, `status(call)`) already returns everything the dashboard needs except the items listed under "Gaps": `agents` (`core.listAgents()`), `messages` (unresolved, capped at `MAX_STATUS_MESSAGES = 200`, with `messagesTruncated`), `stalledAgentIds`, `lostAgentIds`, `stuck`, `inputClears`, `panes`, `reviews`, `integrations`, `agentFindings`, `reports`, `cleanupFailed`, `orphanPanes`, plus the base `ControllerStatus` (`run`, `supervision`, `roles`, `work`, `findings`, `evidence`).
- `cstan status --watch` is the existing client of that route: `callDaemon(socketPath, credential, "status")` from `src/client.ts`, after `ensureRunning(cwd)` in `src/cli.ts`.
- `resolve <messageId> <retry|skip|cancel> [note]` and `cancel <messageId>` are operator routes (`ROUTES` in `src/daemon.ts`; handlers in `src/commands.ts`). The legality rules live in `ControllerCore.resolveMessage` and `resolutionTarget` (`src/controller/messaging.ts`).
- `observe` exists, but it is `access: "agent"` and its handler refuses any caller that is not an active PM or Supervisor agent (`src/commands.ts`, `observe(call)`), and `core.assertCanObserve` checks the `agent:observe` capability. The operator cannot call it today.

## 1. Data source

**Choice: the daemon `status` route over the control socket, not a second SQLite connection.**

- The dashboard process loads the operator context exactly like `status --watch` (`loadOperator` + `ensureRunning`) and calls `callDaemon(socketPath, credential, "status")` on a timer.
- The daemon computes the snapshot on its own connection, inside its own event loop turn. The dashboard opens no database file, takes no lock and writes nothing. It cannot race the controller, and a slow or killed dashboard cannot hold a WAL read transaction open (which would block checkpoints and let the WAL grow).
- Why not read-only SQLite (`openDatabaseReadOnly`): (a) several fields exist only in the daemon's memory, not in the DB: `stalledAgentIds`, `lostAgentIds`, `stuck` come from the driver (`DriverSnapshot` in `src/commands.ts`); (b) it would duplicate the SQL in `statusSnapshot()` and the operator branch and drift from them; (c) it needs the same operator credential anyway. `ControllerCore.openReadOnly` stays what it is today: the offline fallback of `cstan status`.
- **Daemon down:** the dashboard does what `status --watch` does: `ensureRunning(cwd)` at start (same helper, same semantics, same log file). If the daemon stops later, the header shows `controller not answering` in red, the last good frame stays visible and dimmed, and the poll retries with a 1 s, 2 s, 5 s back-off. It never restarts the daemon after start.
- **Polling interval:** default 2 s (same as `status --watch`), `--interval <1..60>` with the same validation as `--watch`. One request in flight at a time (no overlap if the daemon is slow). Request timeout is the existing 5 s socket default of `callDaemon` family; a timeout counts as "not answering", not a crash.
- **Change detection:** the response is hashed (`createHash("sha256")` over `JSON.stringify`); an unchanged hash skips the state update, so Ink does not re-render. The poll is cheap for the daemon: `statusSnapshot()` is a handful of indexed SELECTs per project. `state_version` alone is not enough as a trigger, because messages, stalled flags and input clears change without bumping it.
- **Size limit:** the daemon response frame is capped (1 MiB, `MAX_RESPONSE_FRAME` in `src/control.ts` and the limit in `respond` in `src/daemon.ts`). The dashboard treats a size error as "status too large", shows it in the header and keeps running. Not a new problem: `status --watch` has it today.

**Proof it cannot block or race the controller:** (1) no dashboard-owned DB handle exists; (2) the only call is the existing read-only `status` route, which calls `core.statusSnapshot()` and read queries and mutates nothing; (3) mutations go through daemon commands that run on the daemon's single connection with the core's own versioned, idempotent mutation path; (4) a test asserts that `src/dash/**` never imports `better-sqlite3`, `controller/database` or `controller/core` values (type imports only), see section 7.

### Gaps in the `status` route (additive changes to the operator branch only)

These are the only controller-side changes. Each adds a key to the operator-only part of the result in `src/commands.ts`; none touches `ControllerStatus`, so `CstanStatusJsonV1` and `cstan status` are byte-for-byte unchanged.

| Needed | Today | Proposal |
| --- | --- | --- |
| Health WITH its reason | `supervision.health` only. The reason is stored in the `controller_events` payload of the `supervision.degraded` event (`markSupervisionDegraded`, `src/controller/core.ts`) and nowhere else | Add `supervisionReason: string \| null` to the operator branch: new read method `ControllerCore.supervisionReason(credential)` = newest `controller_events` row with `to_state = 'degraded'` and `entity_type = 'run_control'`, `payload_json.reason`, only when `health = 'degraded'`. Bounded to 2048 chars (the write limit) |
| Worker count vs limit | count = active agents that are not PM or Supervisor (predicate in `src/launcher.ts`, `spawn`); limit = `capstan.toml` `limits.max_workers` | Count computed in the view model from `agents`. Limit read client-side with `loadCapstanConfig(cwd)` (already used by `loadRoleConfig`). If the config is missing or invalid, show `workers N` without a limit. Extract the predicate into one exported helper (`isWorkerKind`) used by both the launcher and the dashboard, so they cannot drift |
| "Working" per agent | Herdr agent status is fetched only inside `launcher.observe` (one Herdr call per agent) | v1 does not call Herdr per poll. `working` is derived: active agent with `lastActivityAt` within 30 s, or the head of its message queue in `sent`/`unacked`. It is labelled as inferred in the help line. See open question 3 |
| Peek at a screen as the operator | `observe` refuses the operator | See action A below |

## 2. Panels and their sources

Layout: one column of boxes, widths follow `useStdout().columns`. Everything is rendered from a pure `DashModel` built from one status response (section 6). All text from the daemon passes through one `clean()` (same rule as `src/watch.ts`: strip control, format and line/paragraph separator characters) because agent-supplied strings (finding text, reasons) are untrusted.

1. **Header.** `run.state` (`status.run`), `supervision.enabled`, `supervision.health` + `supervisionReason` (red when `degraded`, yellow `evaluating`, green `healthy`), supervision epochs (`targetEpoch`/`checkpointEpoch`), `replacementAttempts`, workers `N/limit`, project id. Right side: poll age (`updated 1s ago`), interval, a link-state dot (`●` answering, `○` not answering).
2. **Agents.** One row per element of `agents` (`AgentRecord`: `agentId`, `roleName`, `kind`, `generation`, `state`, `lastActivityAt`). Extra columns: working spinner (inferred, above), `stalled` flag if the id is in `stalledAgentIds`, `lost` if in `lostAgentIds`, pane id from `panes`, queue depth = count of unresolved `messages` to that agent. `blocked` flag = the agent has an unresolved message in a `stuck` entry, or an `escalated` finding targets it. Ended agents are shown dimmed at the bottom, capped at 5.
3. **Report pipeline.** The current (v2) pipeline is three stages, one per table: **Reported** (`reports`, from `agent_reports`: `accepted`/`rejected`) → **Review** (`reviews`, from `reviews`: `started`/`passed`/`findings`/`failed`/`cancelled`) → **Integration** (`integrations`, from `integrations`: `running`/`merged`/`conflicted`/`failed`/`confirmed`/`discarded`). Rendered as counts per stage plus the newest 8 items with state, agent, commit (7 chars) and age. The words "candidate -> verified" in the request belong to the v1 work-item model (`candidates`, `candidate_evidence`, `work_items.state` = `awaiting_verification`...), which `status.work` and `status.evidence` still carry. v1 shows the v2 pipeline as the main panel and the v1 `work` rows in a collapsed secondary panel only when `work.length > 0`. See open question 2.
4. **Message queue and delivery problems.** `messages` (unresolved: any state not in `acked`, `acked_late`, `cancelled`), ordered as the controller orders them. Columns: sequence, recipient, state, age since `queuedAt`, `deferredReason`, `stateReason`, notified marker. A message is a **delivery problem** when its id is in `stuck` (with the reason) or its state is `failed`, `expired` or `unacked`. `messagesTruncated` shows `+more not shown`. Also shown: `inputClears` count (newest 3 with agent and time), `cleanupFailed`, `orphanPanes`. This is the panel the resolve actions work on.
5. **Supervisor findings.** `agentFindings` (open and escalated first): id, target agent, severity, state, `interventions of 2`, `stateReason`; `escalated` rows red with "needs the operator". Same fields `renderWatch` already prints.
6. **Footer.** The key hints for the focused panel and the confirm prompt line (section 5).

## 3. Motion

- **Spinner:** `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 120 ms, one shared timer (`useAnimationTick`), used only for rows that are `working` and for the poll-in-flight dot. ASCII fallback `|/-\`.
- **Sparklines:** `▁▂▃▄▅▆▇█`, 30 samples. Two series kept in a client-side ring buffer: unresolved message count and working agent count, one sample per poll. They start empty when the dashboard starts (the status route has no history). Labelled "since dash start".
- **Change highlights:** a row whose view-model fingerprint changed since the previous frame is rendered inverse/bold for 2 poll cycles, then fades to normal. New stuck/escalated entries also ring the terminal bell once, reusing `signalsOf` from `src/watch.ts` unchanged (that function is exported already) so the bell rule is the same as `status --watch`.
- **NO_COLOR:** if `NO_COLOR` is set (non-empty, per no-color.org), or `--no-color`, or `TERM=dumb`, no colour; meaning is carried by words and symbols (`FAIL`, `!`, `*`) instead of colour alone. Ink's chalk already honours `NO_COLOR`/`FORCE_COLOR`; the view model never encodes meaning only in colour.
- **Reduced motion:** `--reduced-motion` or `CSTAN_REDUCED_MOTION=1`: no spinner (static `*` for working), no highlight fade (changed rows get a `+` marker until the next change), sparklines kept (they are data, not motion) but updated only on poll. The terminal bell is also off.
- **Small terminals:** minimum 60x16. Below that the dashboard shows one line `terminal too small (need 60x16)` and keeps polling. 60 to 99 columns: single column, panels stacked, sparklines hidden, columns dropped in a fixed priority (age, then notified, then pane id). Rows are truncated with `…` (Ink `wrap="truncate-end"`), and each panel shows `+N more` when it does not fit the height. At 100+ columns: two columns (agents + pipeline left, queue + findings right).
- **Resize:** `useStdout()` plus the stdout `resize` event recompute the layout; no extra polling. The alternate screen buffer (`\x1b[?1049h` / `l`) is entered on start and always left on exit, on `SIGINT`/`SIGTERM`, and on an uncaught error (a `finally` plus Ink's `exitOnCtrlC: false` and our own handler), so the user's terminal is never left in the alternate screen with a hidden cursor.

## 4. Non-TTY behaviour

`cstan dash` requires `process.stdin.isTTY && process.stdout.isTTY`. If not, it prints to stderr `cstan: dash needs an interactive terminal; use "cstan status" (or "cstan status --watch") for scripts and pipes` and exits with `EXIT.usage` (2). It does this before loading the operator credential or starting the daemon. `--json` is not accepted by `dash` (usage error pointing to `cstan status --json`). `CAPSTAN_TOKEN`/`CAPSTAN_SOCKET` in the environment (an agent shell) also refuse: `dash` is an operator tool and needs the operator credential, same as `loadOperator` demands for other operator commands.

## 5. Actions, keys, confirm flow

Navigation (read view): `Tab`/`Shift+Tab` or `1`..`5` focus a panel; `↑`/`↓` or `j`/`k` move the row cursor; `?` toggles help; `p` pauses polling (header shows `PAUSED`); `r` forces a poll; `q` or `Ctrl+C` quits. Keys never act without a confirm.

### The exact action list (v1)

| Key | Action | Where | Controller path reused | Notes |
| --- | --- | --- | --- | --- |
| `o` | **Observe** the selected agent's recent screen | Agents panel | New operator-allowed route for the existing observe pipeline, see below | Read-only; opens a full-screen overlay with the sanitized text, `Esc` closes. No state change |
| `y` | **Retry** the selected message | Queue panel | `callDaemon(..., "resolve", [messageId, "retry"])` → `resolve` handler → `core.resolveMessage(ctx, id, "retry")` | Same code as `cstan resolve <id> retry` |
| `s` | **Skip** the selected message | Queue panel | `resolve` with `"skip"` | Same as `cstan resolve <id> skip` |
| `c` | **Cancel** the selected message | Queue panel | `callDaemon(..., "cancel", [messageId])` → `cancel` handler → `core.resolveMessage(ctx, id, "cancel")` | Same as `cstan cancel <id>` |

Excluded from v1 on purpose: `spawn`, `replace`, `release`, `pm-restart`, `launch`, `shutdown`/`stop`, `assign`, `finding` and `integrate`. They run launcher operations of up to 600 s (`LAUNCHER_LIMIT_MS`), kill panes, or change workflow authority; a dashboard keypress is too easy to mistake for them.

Action rules:

- **No bypass.** The dashboard never calls `ControllerCore` and never opens the DB. It only calls `callDaemon` with the operator credential; the daemon applies its own authorization (`ROUTES` access, `core.resolveMessage` rules such as `operator_only`, `recipient_not_active`, `illegal_resolution`, versioned mutation with request idempotency). A refusal comes back as `{ ok: false, error }` and is shown verbatim (cleaned) in the footer.
- **Enable keys only where legal.** Retry/skip/cancel are offered only if `resolutionTarget(decision, state)` (exported from `src/controller/messaging.ts`, pure) is defined for the selected message's state, and retry only when the recipient agent is `active`. This is a hint only; the daemon still decides. Not a second copy of the rule.
- **Confirm flow.** Pressing an action key opens a one-line prompt in the footer: `Retry message 0192… to developer-1 (state sent)? The recipient may already have received it. Type y to confirm, any other key cancels.` Rules: the prompt names the action, the exact id, the recipient and the state; the confirm key is the letter `y` typed as a second, separate keypress (so a held action key or a paste cannot confirm); any other key, `Esc` or a poll that changes the target message's state cancels. For `retry` on `sent`/`unacked` the prompt adds the duplicate-delivery warning that the handler also returns (`warning` in the `resolve` result). The target message id is captured when the prompt opens and is sent as-is, not "the row under the cursor".
- **After the call.** The footer shows the result for 5 s (`resolved 0192…: queued`), then an immediate forced poll refreshes the panel. While a call is in flight further action keys are ignored.
- **No actions while paused, not answering, or when the credential is missing.**

### Action A: observe as the operator (needs a small controller change)

`observe` is agent-only today. The plan adds an operator path without loosening the agent path:

- New route `peek` in `ROUTES` (`src/daemon.ts`): `{ access: "operator" }`. Not a change to `observe`'s access, so the existing `forbidden` behaviour for operators and the existing tests stay valid. Handler `peek(call)` in `src/commands.ts`: args `<agentId> [lines]`, same `SAFE_AGENT_ID` and `parseObserveLines` validation, same `deps.launcher.observe(agentId, lines)` call, same output (`sanitizeScreen`, size caps in `src/observe.ts`), same `note` that the text is unverified. Its own `ReportRateLimiter` (`OBSERVE_RATE_LIMIT`) keyed on `"operator"`. The shared body is extracted from `observe(call)` into one helper, so both handlers use one code path.
- It reads a pane and changes no state (DEC-005 "Rule on reading panes": reading to diagnose is allowed and never changes state or resends). It does not type into any pane.
- Needs no new capability in the core: the operator already passes the route check; the capability check `assertCanObserve` is for agent credentials and is not called for the operator path.
- `cstan peek <agent-id> [lines]` is a free by-product (the route works from the CLI) but is out of scope for this plan unless the user wants it; the dashboard is the only client in v1. See open question 4.

## 6. File layout and CLI dispatch

New files (all under `src/dash/`, tests flat in `test/` because the test script globs `dist/test/*.test.js`):

| File | Purpose | Pure? |
| --- | --- | --- |
| `src/dash/model.ts` | `buildDashModel(status, now, previous?) → DashModel`: panels, rows, flags, fingerprints, worker count, delivery-problem classification | pure |
| `src/dash/format.ts` | `clean`, `age`, `truncate`, `sparkline`, `commitShort`, state→symbol/word maps | pure |
| `src/dash/layout.ts` | `layoutFor(columns, rows) → { mode: "tiny" \| "narrow" \| "wide", visible: … }` | pure |
| `src/dash/actions.ts` | `availableActions(row)`, `confirmText(row, action)`, `toWireCall(action)`; uses `resolutionTarget` | pure |
| `src/dash/poller.ts` | poll loop: interval, single in-flight, hash change detection, back-off; dependencies injected (`fetch`, `sleep`, `now`) like `WatchDeps` | impure, injected |
| `src/dash/app.tsx` | Ink root: state, `useInput`, panels, overlay, footer | Ink |
| `src/dash/components/*.tsx` | `Header`, `AgentsPanel`, `PipelinePanel`, `QueuePanel`, `FindingsPanel`, `Footer`, `PeekOverlay`, `Spinner`, `Sparkline` | Ink |
| `src/dash/run.ts` | entry `runDash(options)`: TTY check, alternate screen, `render(<App/>)`, signal handling, cleanup | impure |

Reused, not reinvented: `loadOperator`, `ensureRunning`, `controllerUnavailable` (`src/cli.ts`; they must be exported or moved to a small shared module, the only edit to existing code in `cli.ts` besides dispatch), `callDaemon` (`src/client.ts`), `signalsOf` and the `clean` rule (`src/watch.ts`; export `clean`), `parseOptions`-style `--interval` validation (extract the block now inline in the `--watch` branch into `parseIntervalSeconds` and use it from both places), `loadCapstanConfig` (`src/config/capstan-config.ts`), `resolutionTarget` (`src/controller/messaging.ts`), `sanitizeScreen` (`src/observe.ts`, server side only).

`src/cli.ts` change: one branch in `runCli`, placed next to the `status` branches, before the final `usage()`:

```
if (command === "dash") return await runDash(parseDashArgs(rest), cwd);
```

with `parseDashArgs` accepting only `--interval N`, `--no-color`, `--reduced-motion` (anything else is `usage()`), and `usage()` text gaining one line. The heavy imports (`ink`, `react`, `src/dash/*`) are loaded with a dynamic `await import("./dash/run.js")` inside that branch, so every other `cstan` command (including the agent hot paths `inbox`, `ack`, `report`) pays no start-up cost for Ink and its yoga WebAssembly module.

### Dependencies and versions (checked 2026-10-02 against the npm registry)

Exact pins, as the repo does for every dependency:

| Package | Version | Where | Why |
| --- | --- | --- | --- |
| `ink` | `7.1.1` | dependencies | `engines.node >=22`, `type: module`, MIT |
| `react` | `19.3.0` | dependencies | ink's peer is `react >=19.2.0` |
| `@types/react` | `19.3.0` | devDependencies | ink's peer is `>=19.2.0` (optional); needed by tsc |
| `ink-testing-library` | `4.0.0` | devDependencies | `engines.node >=18`, `type: module`, peer `@types/react >=18` |

`react-devtools-core` is an optional peer of ink and is not installed. Native or install-script dependencies: none new (`yoga-layout` is WebAssembly, `ws` is pure JS). `ink-spinner` is not used: the spinner is 15 lines and must obey reduced motion.

Compatibility evidence: I installed exactly these four packages in an empty directory on Node v24.6.0 (the repo needs `>=24 <25`), imported them from an `.mjs` ESM file, rendered an Ink tree through `ink-testing-library` and got the expected frame (`"hello\nw"`). This shows ESM import and rendering work on Node 24. It does not show the real dashboard works, nor that the whole 4-package tree passes `npm audit`/licence review; install adds about 26 MB to `node_modules`.

tsc and the build: the build is plain `tsc -p tsconfig.json` with `module: NodeNext`, no bundler. Two choices; this plan takes the first:

1. **`.tsx` with `"jsx": "react-jsx"`** added to `tsconfig.json` (the only tsconfig change). `types: ["node"]` stays; `@types/react` is picked up through imports. `include` already covers `src/**/*.ts`; it becomes `src/**/*.ts`, `src/**/*.tsx`, `test/**/*.ts`, `test/**/*.tsx`. With `verbatimModuleSyntax` and NodeNext, relative imports must keep the `.js` extension (they do already; imports of `./x.js` resolve to `x.tsx`). `jsx` does not change how `.ts` files compile. ESLint (`typescript-eslint`) and Prettier handle `.tsx`; the `lint` and `format` globs are the directories `src test`, so no script change.
2. Alternative with no tsconfig change: write components with `React.createElement` in `.ts` files. Rejected: unreadable at this size.

Strictness note: `exactOptionalPropertyTypes` and `noUncheckedIndexedAccess` apply to the components too; props with optional fields must be written accordingly (cost is minor, noted as a review item).

## 7. Test strategy

Unit tests (no Ink, no sockets), in `test/dash-model.test.ts`, `test/dash-format.test.ts`, `test/dash-layout.test.ts`, `test/dash-actions.test.ts`, run by the existing `node --test dist/test/*.test.js`:

- `buildDashModel` from status objects. Fixtures are built the way `test/watch.test.ts` builds its `status()` helper and from a real status response: the plan reuses `test/harness.ts` (`startDaemonServer`, `createCommandHandlers`, `ControllerCore`, `projectInfo`) to produce one genuine operator status JSON from an in-process daemon, saved once as `test/fixtures/dash-status-*.json` (healthy run; degraded with reason; stuck + stalled + lost; truncated messages; empty project). Real-shape fixtures catch drift between the route and the model.
- Delivery-problem classification, worker counting, stalled/blocked flags, pipeline stage counts, ring buffer, hash change detection, `clean()` against control and bidi characters, sparkline scaling, `age` formatting, layout mode thresholds (59x15, 60x16, 99, 100), truncation with wide (CJK/emoji) characters.
- `availableActions`: for each message state, retry/skip/cancel enablement equals `resolutionTarget` output (table test over all `MessageState` values, so the dashboard cannot drift from the core), the confirm text contains the id, recipient and state, and the duplicate warning appears for `sent`/`unacked`.
- Poller with injected `fetch`/`sleep`/`now`: no overlapping requests, back-off sequence, "not answering" and recovery, unchanged hash causes no emit.
- New daemon route: `test/dash-peek.test.ts` extends the pattern of the existing observe tests (`test/observe.test.ts`) to show the operator succeeds on `peek`, an agent token gets `forbidden`, bad ids and line counts are refused, the rate limit applies, and `observe` for agents is unchanged. `supervisionReason` in the operator status: a degraded project returns the reason; a healthy one returns `null`; a non-operator call does not receive it.

Rendered tests (`test/dash-app.test.tsx`, `ink-testing-library`): `render(<App …/>)` with a fake poller; assert `lastFrame()` for each panel at 60x16, 80x24 and 120x40 (columns are set through the stream object the library provides); key flows through `stdin.write`: focus, cursor, `o` overlay, `y` then wrong key cancels, `y` then `y` calls the injected `callDaemon` exactly once with `["resolve", [id, "retry"]]`, a state change under an open prompt cancels it, actions ignored while paused and while not answering; `NO_COLOR` output contains no ANSI colour sequences; reduced motion shows no spinner frames after advancing fake time. Fake timers via `node:test` `mock.timers` for the spinner and fade.

Safety tests: a static test reads the files in `src/dash/` and fails if any imports `better-sqlite3`, `controller/database`, or a value (not `import type`) from `controller/core`; non-TTY test spawns `dist/src/cli.js dash` with stdio pipes and expects exit code 2 and the message naming `cstan status`; `cstan status` output snapshot test (`test/cli.test.ts` already covers it) must pass unmodified, which shows the status contract did not change.

Not covered by automation: a real terminal's alternate-screen restore and resize, real Herdr `peek`. A manual check list goes into the PR (run in a real terminal with a live daemon, kill the daemon while the dashboard runs, resize, `Ctrl+C`), and `GATES.json` is not changed: `npm run check` already runs the new tests.

## 8. Risks and open questions

Risks:

1. **Controller surface grows.** Two additive changes (`peek` route, `supervisionReason`) touch `src/commands.ts`, `src/daemon.ts` and one new core read method. Mitigation: they are read-only, operator-only and have their own tests; `status` JSON for `cstan status` is untouched.
2. **Inferred "working".** Without a Herdr status per agent the spinner can be wrong (an agent thinking for 40 s shows idle). It is labelled as inferred.
3. **Large status.** Many reports, reviews and messages push the response near the 1 MiB frame cap. The route already caps each list; the dashboard shows an error instead of crashing.
4. **Terminal hygiene.** A crash while in the alternate screen leaves the terminal broken. Mitigated by cleanup on every exit path and a test that exercises exit paths (rendered test checks the restore sequences are written).
5. **New dependency weight and supply chain.** Four packages, 26 MB, about 20 transitive packages; the repo pins exact versions and ships no bundler. Ink 7 needs React 19.2+, which is new to this repo.
6. **Operator credential in a long-lived process.** It is held in memory only, as in `status --watch`; never rendered, never logged.
7. **ink-testing-library 4.0.0 with ink 7.1.1** passed a smoke render, but its last release predates ink 7; if a future ink changes internals, rendered tests are the first to break. Unit tests carry most coverage so this stays contained.

Answers from the user (all eight open questions resolved):

1. Add the operator-only `peek` route; its body is extracted from `observe`, and `observe` is unchanged.
2. The main pipeline panel is reported -> review -> integrated. v1 `work` rows are a secondary panel, shown only when present.
3. "Working" is inferred (activity within 30 s or a message in flight) and labelled as inferred.
4. No `cstan peek` CLI command. The route is dashboard-internal in v1.
5. Retry confirm is a second `y` keypress plus the duplicate-delivery warning. No extra id typing.
6. `"jsx": "react-jsx"`, `.tsx` files and `src/dash/` are accepted.
7. Minimum terminal 60x16; two-column layout from 100 columns.
8. `--interval` is identical to `status --watch` (1 to 60 seconds, default 2).

Review note: `callDaemon` already has a 5 s default timeout (`DEFAULT_TIMEOUT_MS` in `src/client.ts`), so the poller relies on it and does not add its own.

## Consequences

If accepted: `cstan dash` ships in one PR with the two small route/field additions, the `src/dash/` tree, four pinned dependencies, the tsconfig `jsx` setting and the tests above. `cstan status`, `cstan status --watch`, `--json` output, the wire contracts of the existing daemon commands and the SQLite schema (no migration) are unchanged. If a later client wants history (sparklines across restarts), it needs a new daemon route over `controller_events`; this plan does not add one.
