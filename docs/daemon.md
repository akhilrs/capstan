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
check, the version check, and the test-map check. With `CSTAN_LIVE=1` it also runs the live suites. `CARGO_TARGET_DIR` and
`CSTAN_CHECK_JOBS` (cargo's `-j`) are honoured.

The daemon's own tests are the strict transcript replay (`rust/crates/daemon/tests/replay.rs`) of the frozen Node corpus, the
loops, Herdr, launcher and Operator replays, and the black-box suites of `rust/crates/blackbox` that run the built `cstan`.
**Strict is the default** (`tests/common/strict.rs`): any pending step fails. Set `CAPSTAN_DAEMON_PARITY_STRICT=0` to look at one
step. Where Rust differs from a case of the corpus on purpose, an overlay next to the harness replaces that case
(`rust/crates/parity-overlay/README.md`; the `[daemon]` cases are in `rust/crates/config/tests/divergences`).

## Shadow runs (by hand, while the Node tree exists)

The shadow run compares against the frozen Node daemon, so it needs node and `npm run build`; it is not part of `scripts/check.sh`.

`node scripts/shadow-daemon.mjs <ledger-or-project>` compares the two daemons on a real ledger:

1. refuses a source whose daemon is running (a live pid file, or a socket that answers) and a ledger without its project;
2. copies the ledger twice with the SQLite backup API into `/tmp/capstan-shadow-*/{node,rust}` (it never opens the source
   itself);
3. starts the Node daemon on the first copy with `CAPSTAN_LAUNCH=off`, sends a battery of wire requests and records them;
4. starts the Rust daemon on the second copy and replays the transcript;
5. diffs the responses and a dump of every table. Ids and times differ between runs, so every uuid a daemon made and every
   ISO time is replaced by a placeholder numbered in order of appearance; ids already in the source stay as they are.

`--generate <dir>` writes a large scratch project (default 50 agents, 5000 messages, 20 plans with reports and integrations),
`--self-test` generates a small one and shadows it. Never point it at a project whose daemon runs, and never at the live
`.capstan` of a project you are working in: copy a stopped project first. The large run is
`cargo test -p capstan-daemon --test shadow -- --ignored`.

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

## Known limits

- `bind()` of the control socket narrows the process-wide umask for a moment; the Operator's timer thread starts before the
  server, so a file another thread creates in that window gets a narrower mode. Nothing else changes the umask.
- `ProjectGit` repeats the git helper of the replay harness (`tests/common/replay.rs`); the two are meant to be one.
- The replay barrier polls with a 20 second deadline and an idle-connection step sleeps 200 ms; if either flakes, replace the
  sleep with a log or state barrier.
