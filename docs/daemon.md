# The daemon

The controller daemon is Rust. `cstan daemon` (started for you by `cstan start` and by every command that needs a daemon)
serves the project's control socket, the wire protocol and the ledger. `cstan-daemon` is a thin binary that runs the same code
(`cstan-daemon` with no arguments is `cstan daemon`). There is no other implementation. This page is how it is wired, what
became of the old `[daemon]` setting, and how it is checked.

## One daemon, no setting

The Node daemon was removed, so nothing selects a daemon any more.

| Where | Setting | What happens |
| --- | --- | --- |
| `capstan.toml` | no `[daemon]` table | nothing; this is what `cstan init` writes |
| `capstan.toml` | `[daemon]` with `implementation = "rust"` | accepted and ignored, with a warning |
| `capstan.toml` | `[daemon]` with `implementation = "node"` | a configuration error (exit 3) |
| `capstan.toml` | `[daemon]` with no `implementation` | accepted and ignored, without a warning |
| `capstan.toml` | any other value, or a value that is not text | the error `daemon.implementation must be one of node, rust` |
| `capstan.toml` | any other key in `[daemon]`, or `daemon` that is not a table | refused as before |
| environment | `CSTAN_DAEMON=node` | refused (exit 3) |
| environment | `CSTAN_DAEMON=rust` | accepted and ignored, with a warning on standard error |
| environment | any other non-empty value | `CSTAN_DAEMON must be one of node, rust` (exit 3) |

The exact texts:

- warning, printed by `cstan config check` (it is one of the `warnings` of the resolved configuration) and logged by the daemon
  as a `daemon:config_warning` line:
  `[daemon] implementation = "rust" is ignored: Rust is the only daemon; delete the [daemon] table from capstan.toml`
- error:
  `daemon.implementation = "node" is no longer available: the Node daemon was removed and Rust is the only daemon; delete the [daemon] table from capstan.toml`
- `CSTAN_DAEMON=node`:
  `CSTAN_DAEMON=node is no longer available: the Node daemon was removed and Rust is the only daemon; unset CSTAN_DAEMON`
- `CSTAN_DAEMON=rust`:
  `cstan: CSTAN_DAEMON=rust is ignored: Rust is the only daemon; unset CSTAN_DAEMON`

The resolved configuration (`cstan config check`) no longer has a `daemon` member. The variable is read by `cstan start` and by
the commands that start or need a daemon. To migrate, delete the `[daemon]` table and unset `CSTAN_DAEMON`.

### The binary

`cstan` starts the daemon as `cstan daemon`. `CSTAN_DAEMON_BIN` is no longer read.

### The agents' `cstan`

The per-agent `cstan` wrapper the launcher puts first on every agent's `PATH` always runs the `cstan` executable: the running
`cstan` when the daemon runs as `cstan daemon`, and the `cstan` beside the binary when the daemon runs as `cstan-daemon`. A
`cstan-daemon` with no `cstan` beside it fails the launch with `cstan-daemon has no cstan beside it ...` instead of writing a
wrapper that would exec the daemon (with `CAPSTAN_LAUNCH=off` nothing is written and it does not matter). The wrapper holds no
other variable.

## What the daemon wires

`main.rs` builds the real services around `run_daemon_with` (`rust/crates/daemon/src/ports.rs`):

- the Herdr adapter (`capstan-herdr`) for the configured session, with the project slug;
- the launcher (`capstan-launcher`) over the kernel (`KernelHandle` is its `LedgerPort`), the role sync and the log;
- the Operator service and the restart coordinator (`capstan-operator`) when `[operator]` is enabled, with the restart
  result poll and the known-good snapshot; the restart helper is `cstan __restart-helper <plan>` (or `cstan-daemon __restart-helper <plan>`);
- the loops (`RustLoops`): delivery driver, supervision, report relay, review and integration recovery;
- git for the project (`ProjectGit`).

`CAPSTAN_LAUNCH=off` (also `0`, `false`, `no`) keeps the daemon away from Herdr: no adapter, no launcher, no
Operator, no driver.

## The checks

`scripts/check.sh` is the single gate and needs no node: `cargo fmt --check`, `clippy -D warnings` and `cargo test --locked` for
`rust/`, `dash/` and `tools/release`, then the smoke test of the release binaries, the installer test, the release workflow
check, the version check, and the no-Node guard (no tracked TypeScript, `.mjs` or npm file outside `scripts/node-allowlist`). The smoke test runs on the release binaries only when they are built (`CSTAN_RELEASE_BUILD=1` or `scripts/check.sh release`); the Release workflow builds and verifies them on tags. `scripts/check.sh <stage>` runs one stage (`rust`, `dash`, `tools`, `scripts`, `release`, `node-guard`), and CI runs one parallel job per stage. With `CSTAN_LIVE=1` it also runs the live suites. `CARGO_TARGET_DIR` and
`CSTAN_CHECK_JOBS` (cargo's `-j`) are honoured.

The daemon's own tests are the strict transcript replay (`rust/crates/daemon/tests/replay.rs`) of the frozen Node corpus, the
loops, Herdr, launcher and Operator replays, and the black-box suites of `rust/crates/blackbox` that run the built `cstan`.
**Strict is the default** (`tests/common/strict.rs`): any pending step fails. Set `CAPSTAN_DAEMON_PARITY_STRICT=0` to look at one
step. Where Rust differs from a case of the corpus on purpose, an overlay next to the harness replaces that case
(`rust/crates/parity-overlay/README.md`; the `[daemon]` cases are in `rust/crates/config/tests/divergences`).

## Shadow run

The Node shadow tool was deleted with the Node tree. The large generated-project run that
remains is Rust: `cargo test -p capstan-daemon --test shadow -- --ignored`. Never point a shadow run at the live `.capstan` of a
project you are working in.

## Live test

`rust/crates/daemon/tests/live.rs` drives a real Herdr server in a throwaway `capstan-test-*` session (a scratch `HOME`; the
operator's own sessions are never touched) with a stand-in for Claude Code. By hand:

```
cargo build -p cstan-front                        # and cstan-dash in dash/ for the frame check
CSTAN_LIVE=1 cargo test -p capstan-daemon --test live -- --ignored --test-threads=1
```

It starts a PM, spawns, replaces and releases a developer, delivers mail, checks the PM wake, checks that teardown closes
only the project's panes, stops the daemon and removes the session; and restarts the daemon through the Operator
(`op propose --restart`, `op decide`), then runs `cstan` and one frame of `cstan-dash` against the new daemon.

## Ledger retention

The daemon deletes the stored results of old mutation requests. `[ledger] retention_days` (integer 0 to 3650, default 30; 0
turns it off and no loop starts) sets how many days a `mutation_requests` row is kept. About five minutes after the daemon
starts and then every hour, a pass deletes the rows whose `created_at` is older than that, in batches of at most 1000 rows,
each batch one short job on the kernel thread with a 50 ms pause after it. The deletion is not a mutation: it writes no
`controller_events` row and no `mutation_requests` row and leaves `state_version` alone. It logs `daemon:ledger_retention_start`
at start and `daemon:ledger_pruned` with the total for each pass that deleted something.

**The default is on.** The first daemon start after the upgrade deletes the rows older than 30 days, which cannot be undone. To
keep them, set `[ledger] retention_days = 0` before upgrading. A ledger with a long history can have most of the table to
delete, and the first pass takes several minutes of short batches.

### What a replay does after the window

1. Exact replay of a stored result works only while its row exists, so it ends at retention expiry.
2. A request that reaches `Kernel::mutate` with a pruned idempotency key misses the lookup and then fails the
   `expected_version` check with `StateVersionConflict`, because the original commit moved `state_version` past it. It writes
   nothing.
3. The `request_id` uniqueness check also holds only within the window: after a prune, a different mutation that reuses an old
   `request_id` with the current `expected_version` succeeds. This is safe in practice because every non-test caller builds a
   fresh uuid per call for both ids (`req-{uuid}` and `idem-{uuid}`). Only the kernel dispatch API of the blackbox parity
   harness takes caller-given ids.
4. Actor registration results are stored encrypted and replayed by decrypting them with the caller's credential. After the
   window a client can no longer recover a credential by replaying its registration and must ask for a new one.
5. The read-only fast paths that consult `has_stored_request` before `mutate` (`end_wait`, `end_wait_as_controller`,
   `advance_messaging`, `record_agent_observation`, and the pane, actor and message paths that do the same) take their no-op
   branch for a pruned key: they may return the current result instead of the stored one, and they write nothing.

No source other than `kernel/src/kernel.rs` reads `mutation_requests`, so the `cstan` CLI, the Operator and the launcher do not
depend on old rows.

### Size

`controller_events` are immutable and are never pruned. There is no migration, so retention needs no free space for a backup.
Freed pages are reused, but the file does not shrink; a manual `VACUUM` is the operator's choice. The size of the WAL is held
only by SQLite's automatic checkpoints, which are best effort and not a size guarantee.

## Known limits

- `bind()` of the control socket narrows the process-wide umask for a moment; the Operator's timer thread starts before the
  server, so a file another thread creates in that window gets a narrower mode. Nothing else changes the umask.
- `ProjectGit` repeats the git helper of the replay harness (`tests/common/replay.rs`); the two are meant to be one.
- The replay barrier polls with a 20 second deadline and an idle-connection step sleeps 200 ms; if either flakes, replace the
  sleep with a log or state barrier.
