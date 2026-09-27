# DEC-004: durable controller core

**Task:** PM-5
**Status:** Accepted after plan review
**Decision:** Implement a small TypeScript controller core backed by SQLite/WAL. Use maintained Node-compatible SQLite and native file-lock bindings. Keep the controller store, transition engine, and M1 bridge adapter explicit.

## Context

M1 proved the selected Herdr-hosted OMP bridge and durable receipt protocol. It did not provide the transactional data model, transition authorization, request idempotency, or exclusive controller ownership required by M2. The repository has no application package or database layer. PM-5 implements this durable core; PM-6 owns the four-seat workflow, local IPC provisioning, and CLI.

The M1 bridge accepts stable `commandId`, `assignmentId`, `attempt`, `generation`, and `prompt` fields. Its `get` and explicit `start` operations support durable acknowledgement and replay reconciliation. A controller timeout cannot prove that a command was not accepted or executed.

## Decision

- Use strict TypeScript and a maintained Node-compatible SQLite binding. Enable SQLite WAL and foreign keys. Store distinct project/input/plan revisions, work/dependencies, seats/sessions/runtime identities/capabilities, assignments/attempts/generations/input bindings, commands/outbox/delivery attempts/receipts, candidates/evidence, findings/corrections/dispositions, events/operator actions/recovery attempts, and usage observations.
- Apply versioned migrations transactionally. Keep transition events and accepted candidate/evidence records immutable. Use the SQLite online backup API for a consistent backup that includes committed WAL state. Migrations are forward-only; restore from a verified backup instead of copying a live database file or attempting an in-place downgrade.
- Authenticate mutations through server-issued credentials. Resolve actor identity and capabilities from the credential store; do not accept actor or role claims from request content. Recheck current authorization before returning an idempotent replay.
- Require a request ID, idempotency key, expected state version, input revision, and work/assignment identity where applicable. Store a canonical request digest and result. Return the saved result for exact replay and reject conflicting reuse before state advancement.
- Encode legal state changes in explicit entity/source/target/role/capability transition rules. Readiness and candidate acceptance require current bound inputs, accepted dependencies, and candidate-bound verification evidence. Bind accepted non-candidate PM/Supervisor report receipts by command ID and receipt hash; include their content in dependent assignment capsules.
- Persist assignment, attempt/generation, input binding, transition events, and queued command in one transaction. Reject a serialized prompt larger than the M1 256 KiB prompt limit or a complete request frame larger than the M1 1 MiB newline-terminated frame limit before changing assignment state. Persist an `attempting` outbox state before network I/O. Use a stable command ID and exact M1 payload. Persist the bridge's durable acknowledgement before sending `start`. Buffer fragmented response and progress frames across socket reads; reject duplicate response frames. Reconcile an uncertain attempt by querying the same command ID. Unknown or absent-after-attempt remains blocked; do not create a new command or writer from a timeout.
- Hold a kernel `flock` on the project lock inode for the mutable controller lifetime. Every mutable store open requires ownership. The operating system releases the lock when the controller exits. Cooperating controllers cannot own the same project simultaneously.

## Accepted trust boundary

The operator explicitly selected protection against **cooperating controllers**. Same-user unlinking or replacement of the project lock path is outside the accepted local trust boundary. PM-5 does not fence an actor that tampers with that path and must not claim otherwise. This decision does not change DEC-003's separately accepted independent same-user Herdr caller risk.

## Consequences

A process that loses its controller lock must stop making mutations and scheduling decisions. An uncertain bridge command retains its assignment and write authority until reconciliation or a later explicit recovery proves containment. This favors safety over automatic retry when the bridge journal or runtime cannot establish command state.

This decision does not authorize a second command authority, terminal scraping, controller-owned OMP RPC, a four-seat workflow, CLI, full controller crash rehydration, or M3 failure-injection campaigns. The real M1 extension contract is tested at the narrow outbox boundary; full Herdr qualification remains a later milestone.
