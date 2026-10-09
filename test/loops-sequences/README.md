# Loops parity sequences

Each `*.json` file here is one group: `{ "sequences": [ ... ] }`. The exporter (`node dist/test/loops-parity-export.js`)
runs every sequence of every file (found by glob) against the real Node loops in process and writes what happened to
`rust/crates/daemon/tests/loops-parity/<group>.json`. The Rust replay (`rust/crates/daemon/tests/loops.rs`) runs the same
ticks against the Rust loops (`rust/crates/daemon/src/loops`) and compares. A new group file needs no change in the
exporter, the staleness test (`test/loops-parity.test.ts`) or the replay.

## Sequence

```json
{
  "name": "pm-wake",
  "seed": "defaults to <group>/<name>",
  "loop": "driver",
  "preset": "team",
  "config": {},
  "pmMail": "pm-1",
  "setup": [],
  "ticks": []
}
```

- `loop` is `driver` (`DeliveryDriver`, with `createNotifier` and a stub process probe), `supervision` (`startSupervision` with
  a stub launcher), `relay` (`startReportRelay`) or `recover` (`recoverIntegrations` with a stub git).
- `preset: "team"` first registers a PM, a developer, a verifier and a supervisor as `pm-1`, `dev-1`, `ver-1` and `sup-1`
  (bound as `$pm`, `$dev`, `$ver` and `$sup`). Without it the sequence builds its own team in `setup`.
- `setup` and a tick's `steps` use the language of `test/kernel-sequences` (`op`, `context`, `args`, `as`, `$name.path`
  references, `$owner.credential`, `$internal.credential`) on one `ControllerCore` per sequence, opened with the seeded
  clock and randomness of `test/kernel-parity-hooks.ts`. The Rust replay runs the recorded, resolved steps.
- `config`: `timers` (over the defaults of one hour each; a value like `0.001` makes a timer due at once, because the seeded
  clock moves one millisecond per reading), `pmStaleSeconds`, `tickMs`, `channels` (`{ herdr, fallback }`), `supervision`
  (`{ enabled, checkSeconds }`), `supervisorRole` (null: none), `findingCheckSeconds` (the relay's finding sweep).
- `pmMail`: the agent whose open mail is summarised with `pmMailSummary` after each tick.

## Ticks

A tick is `{ at?, steps?, adapter?, dump?, repeat? }`. `at` is the loop's own time in milliseconds after the seeded epoch
(the driver's and the supervision's `now`; default: the previous plus `tickMs`). `repeat` runs the tick that many times in
a row. The driver and supervision take their time from `at`; the relay has no clock option and reads `Date.now()`, which the
seeded clock answers, one reading per use. `adapter` patches what the stubs answer; each section is merged key by key into
the one before and a `null` removes a key:

- `panes` agentId -> paneId; `entries` paneId -> the agent its registry entry names (null: no entry; default: the agent that
  has the pane); `observe` agentId -> a state or `{ "error": spec }`.
- `send` paneId -> `"sent"`, `{ "deferred": reason, "detail"?, "blocker"? }`, `{ "error": spec }` or
  `{ "hookThenError": spec }` (the ledger record is made, then the send fails). `wake` paneId -> `"sent"`, `"pm_not_idle"`,
  `"input_not_empty"`, `{ "error" }` or `{ "hookThenError" }`. `clear` paneId -> `{ "cleared", "text"?, "keys"? }` or
  `{ "error" }`. `probe` paneId -> `{ "processes": [{ pid, ppid, comm, cpuMs }] }` or `{ "error" }`.
- `notify` `"ok"` or `{ "error" }` for the Herdr channel; `spawn` and `release` likewise for the launcher.
- `git` (recover): `tips` branch -> sha, null or `{ "error" }`; `delete` branch -> bool or `{ "error" }`; `covered` -> the
  reports git says are covered, or `{ "error" }`.

An error `spec` is `Class`, `Class:message`, `HerdrError:code[:message]`, `LauncherError:code[:message]` or
`InputUnreadable:blocker[:message]`, where `Class` is one of the adapter's error classes or `Error`.

## Output

Per tick the export records `at`, the resolved `steps` and `adapter`, then what the loop did: `calls` (every adapter,
notifier, probe, launcher or git call, as `method arguments` lines), `log` (the loop's log events), `coreCalls` (the
`ControllerCore` members it used, counted by a proxy: the Rust loops make exactly these calls, which carries the statement
and row budgets of `test/daemon-cost.test.ts` over, since the kernel's methods are proven SQL-equivalent), the driver's
`snapshot`, the `notifications` written, the `pmMail` summary and `tablesDiff` (the ledger rows added and removed by the tick).
