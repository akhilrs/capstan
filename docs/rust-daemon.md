# The Rust daemon (opt-in)

`cstan-daemon` is the Rust port of the controller daemon (`cstan daemon`). It serves the same control socket, the same
wire protocol and the same ledger, and it is **off by default**: a project that says nothing runs the Node daemon exactly
as before. This page is how to turn it on, how it is wired, and the checks that keep it in step with Node.

## Choosing the daemon

| Where | Setting | Wins |
| --- | --- | --- |
| `capstan.toml` | `[daemon]` `implementation = "node"` or `"rust"` | after the environment |
| environment | `CSTAN_DAEMON=node` or `rust` | over the file |
| neither | the Node daemon | |

```toml
[daemon]
implementation = "rust"
```

- The starter `capstan.toml` that `cstan init` writes does **not** contain the table.
- Any other value is a configuration error, with the usual exit code: `cstan config check` prints
  `daemon.implementation must be one of node, rust`; a bad `CSTAN_DAEMON` prints `CSTAN_DAEMON must be one of node, rust`.
  An unknown key in `[daemon]` is refused like an unknown key anywhere else.
- Both configuration readers (Node and `rust/crates/config`) accept `[daemon]` identically; the shared parity fixtures hold
  the cases (`edge-daemon-*`).
- The selection is made when the client spawns a daemon (`daemonCommand` and `ensureDaemon` in `src/client.ts`), so
  `cstan start`, every routed command that starts a daemon, `cstan dash` and the Operator's restart helper go through the
  same choice. A daemon that is already running is not replaced: `cstan stop` it first.
- Nothing changes when the setting is absent: same argv, same environment, same output, same exit codes, same wire fields.

### The binary

`CSTAN_DAEMON_BIN` names it (an absolute path to an executable file). Without it the client looks for `cstan-daemon`
beside the running `cstan` executable (the standalone binary) or in the directory of the CLI file and the one above it
(a source checkout: `dist/` and `dist/src/`). With `implementation = "rust"` and no binary the start fails with
`start_failed` (`the Rust daemon could not be started: ...`). **It never falls back to Node silently.**

### The agents' `cstan`

The daemon is not a CLI. The per-agent `cstan` wrapper the launcher puts on every agent's `PATH` runs the Node CLI that
started the daemon: the client sets `CSTAN_NODE_CLI` (and `CSTAN_NODE`, the Node executable) in the daemon's environment.
`CSTAN_FRONT_END` (the Rust `cstan` front end) is honoured as with the Node daemon.

## What the daemon wires

`main.rs` builds the real services around `run_daemon_with` (`rust/crates/daemon/src/ports.rs`):

- the Herdr adapter (`capstan-herdr`) for the configured session, with the project slug;
- the launcher (`capstan-launcher`) over the kernel (`KernelHandle` is its `LedgerPort`), the role sync and the log;
- the Operator service and the restart coordinator (`capstan-operator`) when `[operator]` is enabled, with the restart
  result poll and the known-good snapshot; the restart helper is `cstan-daemon __restart-helper <plan>`;
- the loops (`RustLoops`): delivery driver, supervision, report relay, review and integration recovery;
- git for the project (`ProjectGit`).

`CAPSTAN_LAUNCH=off` (also `0`, `false`, `no`) keeps the daemon away from Herdr, as in Node: no adapter, no launcher, no
Operator, no driver.

## The parity gate

Three parts keep Node and Rust in step; `npm run check` runs all of them.

1. `npm run check:dash` (cargo): the strict transcript replay (`rust/crates/daemon/tests/replay.rs`), the loops, Herdr,
   launcher and Operator replays. **Strict is the default** (`tests/common/strict.rs`): any pending step fails. Set
   `CAPSTAN_DAEMON_PARITY_STRICT=0` to look at one step.
2. `npm run check:rust-daemon` (`scripts/check-rust-daemon.mjs`): the black-box suites listed in
   `test/rust-daemon-suites.ts` run with `CSTAN_DAEMON=rust` against the release `cstan-daemon` that `check:dash` built
   (`${CARGO_TARGET_DIR:-rust/target}/release/cstan-daemon`; `CSTAN_DAEMON_BIN` names another), then a small shadow run.
   A suite that asserts Node-only internals goes in `RUST_DAEMON_SKIPS` with the reason; it is never edited to pass.
   Skippable like `check:dash`: `CSTAN_SKIP_RUST_DAEMON_CHECK=1`, or `CSTAN_SKIP_DASH_CHECK=1` without cargo.
3. `npm test`: the staleness tests (`config-parity`, `cli-transcript`, ...) fail while a committed Rust fixture differs from
   a fresh Node export; `test/daemon-select.test.ts` covers the selection and the gate's own wiring.

When Node behaviour changes, regenerate the fixtures (`node dist/test/config-parity-export.js`, the kernel, daemon and
CLI exporters) and fix the Rust side until the strict replays pass again.

## Shadow runs

`node scripts/shadow-daemon.mjs <ledger-or-project>` compares the two daemons on a real ledger:

1. refuses a source whose daemon is running (a live pid file, or a socket that answers) and a ledger without its project;
2. copies the ledger twice with the SQLite backup API into `/tmp/capstan-shadow-*/{node,rust}` (it never opens the source
   itself);
3. starts the Node daemon on the first copy with `CAPSTAN_LAUNCH=off`, sends a battery of wire requests and records them;
4. starts `cstan-daemon` on the second copy and replays the transcript;
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
npm run build
cargo build -p cstan-daemon -p cstan-front        # and cstan-dash in dash/ for the frame check
cargo test -p capstan-daemon --test live -- --ignored --test-threads=1
```

It starts a PM, spawns, replaces and releases a developer, delivers mail, checks the PM wake, checks that teardown closes
only the project's panes, stops the daemon and removes the session; and restarts the daemon through the Operator
(`op propose --restart`, `op decide`), then runs the Node client, the Rust `cstan` front end and one frame of `cstan-dash`
against the new daemon.

## Known limits

- `bind()` of the control socket narrows the process-wide umask for a moment; the Operator's timer thread starts before the
  server, so a file another thread creates in that window gets a narrower mode. Nothing else changes the umask.
- `ProjectGit` repeats the git helper of the replay harness (`tests/common/replay.rs`); the two are meant to be one.
- The replay barrier polls with a 20 second deadline and an idle-connection step sleeps 200 ms; if either flakes, replace the
  sleep with a log or state barrier.
