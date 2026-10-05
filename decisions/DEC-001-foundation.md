# DEC-001: independent Capstan controller

**Task:** PM-3  
**Status:** accepted for M1  
**Decision:** Build a small independent TypeScript controller. Use OpenRig as conceptual evidence only.

## Pinned assessment

OpenRig v0.5.14 is pinned at `cc75efdd17fb967bde7cff6c5805791986af78d8`.

| Required capability | Evidence at pin | Verdict |
| --- | --- | --- |
| OMP integration with Herdr sessions | `packages/daemon/src/adapters/` contains Claude, Codex, Pi, terminal, stub, and cmux adapters. It contains no OMP adapter. README declares Pi/OpenCode adapters in development and tmux as the runtime. | absent |
| Assignment identity, deduplication, durable acknowledgement | `queue-repository.ts` and `outbox-handler.ts` provide idempotent queue/outbox records. | partial; no Capstan assignment attempt/generation contract |
| Atomic handoff and restart reconciliation | `workflow-projector.ts` uses one SQLite transaction to close a packet, create its successor, append trail, update state, and persist events. | partial; not an OMP/Herdr assignment recovery proof |
| Separate Supervisor with routed findings | Watchdog migrations and `watchdog-*` domain modules exist. | partial; no required Capstan finding lifecycle or role authority contract |
| Single writer and stale-report rejection | Queue ownership and workflow version guards exist. | unproven; no Capstan workspace containment or superseded assignment-report contract |

## Rationale

OpenRig's durable queue and transactional-scribe patterns are useful references. Its selected runtime is tmux. The pinned source has no OMP adapter. Extending it would introduce a daemon, migration, and runtime coupling while still requiring Capstan-specific generations, candidate binding, verification, containment, and operator controls. An independent controller isolates the MVP proof and follows section 5.1 of the original MVP plan (since removed; see git history).

## Scope and license

M2 uses TypeScript, SQLite/WAL, local IPC, file-backed artifacts, and a maintained Node-compatible SQLite binding. It does not add a broker, plugin platform, distributed scheduler, or generic workflow language.

OpenRig is Apache-2.0. M0 copies no OpenRig source, schema, package, migration, artifact, or derivative. `NOTICE` was absent at this commit. Any later source adoption requires a new decision, preservation of license and attribution notices, changed-file notices, and any required NOTICE handling.

## Consequences

M1 must prove the OMP/Herdr bridge and Docker containment. It must stop and record a failing property rather than silently switching transport. M2 may reuse the concepts of append-only transitions, idempotent delivery, and transactional handoff without copying implementation.
