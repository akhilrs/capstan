# Parity fixtures and the freeze

Every Node behaviour the Rust port had to keep is recorded as a fixture: a JSON file that an exporter wrote by running the
Node code, and that a Rust test replays and compares byte for byte. The Node tree and its exporters are deleted; the
corpus is **frozen** and is the only reference there is. The fixtures live under `rust/crates/*/tests/**`, `dash/tests/parity`
and the shared inputs in [`rust/testdata/`](../rust/testdata/README.md) (which says which Node file each came from). A fixture
changes only by hand, in the same commit as the Rust change, with the reason in the commit body; nothing regenerates it. The one exception is the `init-*` and `usage-init-*` local transcripts, whose recorded result (the `node` field, `files`, `commits`) is rewritten from the Rust binary after `cstan init` changed (the managed `.gitignore` block, no `roles/` folder): `CSTAN_REGEN_LOCAL=init-,usage-init- cargo test -p cstan-front --test local`, then review the diff. The `config-expected.json` files of the two parity cases `captured-0014` and `captured-0312`, which load the starter, were likewise regenerated from the Rust output: the starter no longer sets `prompt_file` for the designer, and `cstan init` now writes the managed `.gitignore` block.
The Node exporters are in the git history at the commit below if one is ever needed.

The pi host (`pi-130-*.ansi` in [`rust/testdata/fixtures`](../rust/testdata/fixtures)) is the one addition made after the freeze: the six screens (idle empty, typed, after Ctrl-U, multi-line, busy, trust dialog) were captured by hand from pi 1.3.0 in a Herdr pane, as [docs/research/pi-dev-integration.md](research/pi-dev-integration.md) describes, not exported from Node. Their cases in `screen.json`, the `pi` columns of the other screen cases, the `pi` results in `validate.json`, the `pi-*` synthetic screens, the `rust-pi-*` config cases and the `start-pi-agent-and-trust-dialog` adapter scenario were written by hand from the Rust output and reviewed; Node never knew the `pi` kind.

## The freeze point

**`be41e824207a002996822e991aa8b61b90219b47`** (`feat(daemon): Rust daemon port phase 2b: opt-in Rust daemon with Node`)
is the main commit the freeze branch started from, and the commit every exporter below ran on: at it, the staleness test of each exporter re-exported and passed. This commit, not main at the time of merge, is the freeze point. From it on, the committed fixtures
are the reference.

## The exporters

| Exporter (historic) | Fixtures                                                            | Replayed by (Rust)                                                      | Staleness test (historic)          |
| ------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------------- | --------------------------------- |
| `config-parity-export`                | `rust/crates/config/tests/parity`                                   | `config/tests/config_parity.rs`, `prompts_parity.rs`, `differential.rs` | `config-parity.test.ts`           |
| `cli-transcript-export`               | `rust/crates/cstan/tests/transcripts`                               | `cstan/tests/transcripts.rs`                                            | `cli-transcript.test.ts`          |
| `cli-local-transcript-export`         | `rust/crates/cstan/tests/local-transcripts` (inputs: `ledgers/`)    | `cstan/tests/local.rs`                                                  | `cli-local-transcript.test.ts`    |
| `kernel-parity-export`                | `rust/crates/kernel/tests/parity` (and `rust/testdata/fixtures/plan-bodies`) | `kernel/tests/replay.rs`, `strict.rs`                                   | `kernel-parity.test.ts`           |
| `kernel-integrate-export`             | `rust/crates/kernel/tests/parity-integrate`                         | `kernel/tests/integrate.rs`                                             | `kernel-integrate-parity.test.ts` |
| `daemon-transcript-export`            | `rust/crates/daemon/tests/transcripts`                              | `daemon/tests/replay.rs`, `cmds.rs`, `budget.rs`, `shadow.rs`           | `daemon-transcript.test.ts`       |
| `loops-parity-export`                 | `rust/crates/daemon/tests/loops-parity`                             | `daemon/tests/loops.rs`                                                 | `loops-parity.test.ts`            |
| `launcher-parity-export`              | `rust/crates/launcher/tests/parity`                                 | `launcher/tests/parity.rs`, `operations.rs`                             | `launcher-parity.test.ts`         |
| `operator-parity-export`              | `rust/crates/operator/tests/parity`                                 | `operator/tests/parity.rs`                                              | `operator-parity.test.ts`         |
| `herdr-parity-export`                 | `rust/crates/herdr/tests/parity`                                    | `herdr/tests/parity.rs`, `adapter_replay.rs`                            | `herdr-parity.test.ts`            |
| `dash-parity-export`                  | `dash/tests/parity`                                                 | `dash/tests/model_parity.rs`, `view_parity.rs`, `e2e_parity.rs`         | `dash-parity.test.ts`             |
| `ledger-parity-export`                | `rust/crates/ledger/tests/parity`                                   | `ledger/tests/migrate.rs`, `refusals.rs`                                | `ledger-rust-interop.test.ts`     |

The first ten rows are the exporters the freeze check ran. Each staleness test (deleted with the Node tree) built a fresh export and failed while the
committed files differ from it, or while a file is missing or unexpected.

## The CLI corpus

The commands of `cstan` are recorded by two exporters. A transcript holds `argv`, `env`, the `layout` of the scratch
directory, `cwd`, the daemon's reply or a scratch daemon, and what Node printed: `node.stdout`, `node.stderr`,
`node.exit`.

- **`cli-transcript-export`** (`transcripts/`, 328 files): the commands the Rust front end answers or hands to Node,
  against a fake daemon that sends one fixed reply. The 80 transcripts marked `fallback` hold Node's output under
  `node`; the CLI-native packages compare against that.
- **`cli-local-transcript-export`** (`local-transcripts/`): the commands that run on the local machine or against a real
  Node daemon of a scratch project started with `CAPSTAN_LAUNCH=off`. They add `stdin`, `tty` (run on a pseudo-terminal,
  stdout and stderr merged), `before` (the commands that bring the project to its starting state), `stopAfter` (a watch
  that never ends is stopped once its stdout holds this text), and `files` and `commits` (the tree, modes and bytes a
  command created, and the commits of each git work tree afterwards).

Each local transcript is one of: version, help and every usage error; `init` with and without `--git`, in a repository,
in an empty one, without git, with `.capstan` in the way; `config check` and `config sync`, valid and invalid;
`herdr-config`; `daemon` with extra arguments; `start` and `stop` against a scratch daemon (fresh start, already running,
unreachable, refused, launch failed); `status --watch` (first frame, bad `--interval`); the refusals of `dash` (not a
terminal, agent shell, bad flags); offline `status` and `inspect`, text and `--json`, over the committed ledgers at
migration versions 20, 30, 36 and 37 (`local-transcripts/ledgers`); `inbox --hook`; and one transcript for every
`ROUTES` command of the Node daemon in the operator environment and, where its access allows, the agent environment.
Its staleness test enumerated `ROUTES` and failed on a command with no transcript.

### Normalisation

It followed the Node CLI parity harness: a value of a kind is replaced by a placeholder that keeps the kind, and
everything else is exact.

| In                         | Placeholder                                                 |
| -------------------------- | ----------------------------------------------------------- |
| inputs (argv, env, layout) | `$ROOT` the scratch directory, `$PATH` the host's tool path |
| scratch paths in output    | `<ROOT>`                                                    |
| ids                        | `<ID#n>` by order of first appearance                       |
| timestamps                 | `<TS:z>` (UTC), `<TS:local>`                                |
| pids, ages                 | `<PID>`, `<AGE>`                                            |
| the operator key           | `<TOKEN:operator>`                                          |
| the package version        | `<VERSION>`                                                 |

`<VERSION>` is the package version as a whole token, in every transcript of both exporters, so a release bump changes no
fixture. The Rust replay substitutes the version the binary was built with. `__front-version` cannot be run by Node, so
its transcripts carry the output the front end must print in `expected`.

The ledgers under `local-transcripts/ledgers` are inputs, not exports: a ledger at the current schema with one project, its
operator and one work item, and the same rows cut back to the first 20, 30 and 36 migrations. Their exporter is gone.

## After the freeze

1. The freeze point is the main commit the freeze branch started from, and the exporters ran on it. From then the
   committed fixtures are the reference.
2. The version is normalised in the transcripts (`<VERSION>`), so a release bump changes no fixture.
3. Committed fixtures are edited only by hand, as rule 7 says.
4. A deliberate Rust divergence is an overlay: one JSON file per changed case under
   `rust/crates/<crate>/tests/divergences/`, naming the fixture, the expected Rust output and the reason. The schema and
   the reader are in the `capstan-parity-overlay` crate: see [`rust/crates/parity-overlay/README.md`](../rust/crates/parity-overlay/README.md).
   The Rust replay reads the overlay in place of the Node expectation, and an overlay replaces the whole expected output of
   a case. The `cstan` replays (`cstan/tests/transcripts.rs`, `local.rs`) read `rust/crates/cstan/tests/divergences`
   through `capstan-parity-overlay`; an overlay there names its fixture as `transcripts/<file>` or
   `local-transcripts/<file>`, relative to `rust/crates/cstan/tests`, and the case is the transcript's `name`.
5. The 80 `cstan` transcripts marked `fallback` already hold Node's output under `node`, and cli-native compares against
   that.
6. No new Node-generated fixtures are made.
7. After Node removal a fixture changes only by hand, in the same commit as the Rust change, with the reason in the commit
   body.
8. Fixtures stay where the Rust tests read them (`rust/crates/*/tests/**`, `rust/testdata/`).
9. The Node tree is deleted (plan D); the corpus above is what remains of it.

## Divergences from Node

Where Rust deliberately differs from Node, either an overlay (above) replaces the expectation of a Node fixture, or, when
Node has no fixture for the behaviour, a Rust test holds it and is listed here.

| Where                                 | What differs                                                                                                                                                                                                                     | Held by                                         |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `cstan dash` finds no `cstan-dash`    | Rust-only, no Node fixture reaches it (Node ran its Node dashboard): one line, `cstan: cstan-dash was not found; install it beside cstan, put it on PATH or set CSTAN_DASH_BIN to its absolute path (docs/reference/install.md)`, and the usage exit code 2 | `rust/crates/cstan/tests/dash_handoff.rs`       |
| The `[daemon]` table and `CSTAN_DAEMON` (config cases `edge-daemon-node`, `edge-daemon-rust`, `edge-daemon-empty`) | Retired at cutover: overlays in `rust/crates/config/tests/divergences` replace the three cases (an error for `node`, a warning for `rust`, no `daemon` member in the resolved JSON); `CSTAN_DAEMON` has black-box tests (`cli::cstan_daemon_node_is_refused_and_rust_warns`). The Node-versus-Rust live tests (`config-parity`, `cli-parity`) have no `[daemon]` case, so none is skipped. The exact texts are in `docs/daemon.md`. |
| Host-switchable roles (config cases `captured-0202`, `captured-0203`, `captured-0204`, `captured-0230`, `captured-0533`, `rust-pi-allow`, `rust-pi-deny`) | `allow`, `deny` and `mcp` on a codex, omp or pi role are ignored with a warning instead of refused, and a role with no `permission_mode` on those hosts takes `auto`; Node refused all seven. Seven overlays in `rust/crates/config/tests/divergences` (reasons in its README); `hooks = "inherit"` is a warning only and changes no hash. The `config-check` transcripts of the codex and pi full-access cases gained the no-model warning, and `local-transcripts/config-check-codex-warning.json` now loads (exit 0). |
