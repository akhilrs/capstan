# Kernel parity sequences

Each `*.json` file here is one group: `{ "sequences": [ ... ] }`. The exporter (`node dist/test/kernel-parity-export.js`)
runs every sequence of every file (found by glob) against the real `ControllerCore` with seeded time and randomness
(`test/kernel-parity-hooks.ts`) and writes the resolved steps and what happened to
`rust/crates/kernel/tests/parity/<group>.json`. The Rust replay (`rust/crates/kernel/tests/replay.rs`) runs the same
steps on `capstan-kernel` and compares. A new group file needs no change in the exporter, the staleness test or the
replay.

## Sequence

```json
{
  "name": "actors",
  "seed": "defaults to the name",
  "project": { "name": "overrides of the default project" },
  "requires": ["messages"],
  "open": true,
  "steps": []
}
```

A sequence runs in its own scratch directory (`tempDir`), with a clock that starts at 2026-01-01T00:00:00.000Z and ticks
1 ms per reading and a randomness stream seeded by `seed`. Unless `"open": false`, the exporter first opens a controller
(handle `main`) on a default project whose owner credential is `$owner.credential`. `requires` lists the groups other than
the sequence's own whose operations it reaches: when the Rust kernel answers a step with `Unported`, a sequence with
`requires` is reported as pending (a failure under `CAPSTAN_KERNEL_PARITY_STRICT=1`), a sequence without it fails.

## Steps

```json
{
  "op": "createActor",
  "context": { "credential": "$owner.credential" },
  "args": [{ "displayName": "x" }],
  "as": "a1"
}
```

- `op` is the `ControllerCore` method name (the Rust method is its snake_case; the dispatcher takes the camelCase name),
  or `open`, `openReadOnly` (`args[0]` is `{ "project"?, "stateDir"?, "options"? }`: project overrides, `"dir"` or a literal
  state directory, `workspaceRoot`/`runtimeWorkspacePath`), `close`, or `dump` (record every table of the ledger now).
  Getters (`projectId`, `stateVersion`, `inputRevision`) are ops without arguments.
- `context` is a symbolic `MutationContext`: it becomes the first argument. `credential` is required;
  `requestId` and `idempotencyKey` default to `req-<step index>` and `idem-<step index>`, `expectedVersion` and
  `inputRevision` to the handle's current counters. Set any of them to make a replay or a conflict.
- `args` follow the context. Any string `"$name.path"` is replaced by that field of the result of an earlier step
  named by `as`; `$owner.credential` and `$internal.credential` (the internal controller's credential, knowable because
  the seeded stream is) are predefined; `$version` and `$revision` are the handle's counters.
- `on` names the controller handle (default `main`); `dump: true` records the tables after the step as well.

## Output

For every step the export records `op`, `on`, the resolved `context` and `args`, then `result` or `error` (the error class
name) with `message`, and the project's `stateVersion` and `inputRevision` while the handle is open. After the last step it
records `tables`: every table of the ledger, rows ordered by all columns (`schema_migrations.applied_at` left out,
because the migration timestamps come from the system clock in both implementations; the ledger is migrated before the
first open for the same reason).
