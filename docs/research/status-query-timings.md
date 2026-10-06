# Status query timings after migration 0035

Package plan-22/db-status. Follow-up to `dash-performance.md`: index the `roles` query, make `listAgents` one query and cache prepared statements.

## Changes

- Migration `0035_status_query_indexes.sql`: `actors_by_seat_active (project_id, seat_id, active)` and `assignments_by_seat_authority (project_id, seat_id, authority_state, created_at)`. The `roles` query now plans as `SEARCH a USING INDEX actors_by_seat_active` and `SEARCH a USING COVERING INDEX assignments_by_seat_authority`; before it scanned `actors` by `project_id` for every seat.
- `listAgents()` runs one `SELECT` over `agents WHERE project_id = ? ORDER BY agent_id` and maps rows with the same function as `agentRecord` (was one re-prepared `SELECT *` per agent).
- `Database.prepare(sql)` reuses one `StatementSync` per SQL text (256 entries, least recently used dropped, cleared on close). No caller uses `iterate()`; the adapter has none.

## Method

Scratch project `/tmp/cb` (`cstan init` in a fresh git repo, `herdr` off `PATH`, `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` unset, never the live daemon or ledger). Seeded through `ControllerCore` with 450 developer agents, 451 seats/actors (the PM plus 450 developers). The ledger has **no messages, reports or assignments**, unlike the 3000-message ledger of `dash-performance.md`, so absolute numbers are not directly comparable to that report; the before figures below were measured on this same ledger.

## Results (450 agents, 451 seats)

| Measurement                                                                    | Before                                          | After                            | Gate            |
| ------------------------------------------------------------------------------ | ----------------------------------------------- | -------------------------------- | --------------- |
| `roles` query, `query-timing.mjs` (ledger copy with / without the two indexes) | 33.0 ms                                         | **1.7 ms**                       | <= 3 ms         |
| `listAgents()` through `ControllerCore`                                        | 28.0 ms (N+1, prepare each; `query-timing.mjs`) | **2.2 to 2.4 ms** (5 runs of 30) | <= 3 ms         |
| `statusSnapshot()` through `ControllerCore`                                    | n/a                                             | 2.4 to 2.6 ms                    | n/a             |
| `status-probe.mjs` p50 over 100 requests (daemon, idle)                        | ~160 ms (report, other ledger)                  | **13.2 ms** (min 12.0, max 27.3) | target <= 40 ms |
| `dash-bench.py` daemon CPU, 0 dashes, 2 s, 60 s                                | 6.2% (report, other ledger)                     | 0.4%                             | recorded        |
| `dash-bench.py` daemon CPU, 1 dash, 2 s, 60 s                                  | ~8 points per dash (report)                     | 1.1%                             | recorded        |

The probe and dash-bench targets are met, so no `--cpu-prof` profile of the remaining costs was taken.

## What I could not verify

- The before column for the probe and the daemon CPU is the report's number on its richer ledger; I did not rebuild the old code on this ledger. The `roles` before figure is the same ledger without the indexes; the `listAgents` before figure is the old N+1 loop timed by `query-timing.mjs`. The report's 63.4 ms for `roles` came from a ledger with more rows per seat, which makes the unindexed scan slower.
- Single machine, one run per daemon cell; the live ledger (messages, plans, integrations) is not covered.
