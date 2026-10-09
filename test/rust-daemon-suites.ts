/**
 * The black-box suites that `npm run check:rust-daemon` runs with CSTAN_DAEMON=rust against the release cstan-daemon, and the
 * ones it leaves out with the reason. A suite that asserts Node-only internals is listed here, never edited to pass.
 *
 * Most of these suites start the daemon in process or through the CLI; with CSTAN_DAEMON=rust every one that goes through
 * `cstan start`, `ensureDaemon` or a routed command talks to the Rust daemon, the rest keep checking the Node code they
 * share with the front end. Together with `check:dash` (cargo test: the strict transcript, loops, herdr, launcher and
 * operator replays) and the staleness tests of `npm test` this is the gate that keeps Node and Rust in step.
 */
export const RUST_DAEMON_SUITES: readonly string[] = [
  "daemon.test",
  "daemon-background.test",
  "daemon-operator.test",
  "cli.test",
  "operator-commands.test",
  "plan-flow.test",
  "dash-poller.test",
];

/** Suite name -> why the Rust daemon is not run through it. */
export const RUST_DAEMON_SKIPS: Readonly<Record<string, string>> = {};
