# Parity fixtures and the freeze

Every Node behaviour the Rust port has to keep is recorded as a fixture: a JSON file that an exporter in `test/` wrote
by running the Node code, and that a Rust test replays and compares byte for byte. The Rust port was built against
these fixtures, and once Node is deleted they are the only reference there is.

## The freeze point

**`be41e824207a002996822e991aa8b61b90219b47`** (`feat(daemon): Rust daemon port phase 2b: opt-in Rust daemon with Node`)
is the main commit the freeze branch started from, and the commit every exporter below ran on: at it,
`npm run build && npm test` runs the staleness test of each exporter, which re-exports and fails on any difference, and
all of them pass. This commit, not main at the time of merge, is the freeze point. From it on, the committed fixtures
are the reference.

## The exporters

| Exporter (`node dist/test/<name>.js`) | Fixtures                                                            | Replayed by (Rust)                                                      | Staleness test (`test/`)          |
| ------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------------- | --------------------------------- |
| `config-parity-export`                | `rust/crates/config/tests/parity`                                   | `config/tests/config_parity.rs`, `prompts_parity.rs`, `differential.rs` | `config-parity.test.ts`           |
| `cli-transcript-export`               | `rust/crates/cstan/tests/transcripts`                               | `cstan/tests/transcripts.rs`                                            | `cli-transcript.test.ts`          |
| `cli-local-transcript-export`         | `rust/crates/cstan/tests/local-transcripts` (inputs: `ledgers/`)    | none yet: the CLI-native packages add the replay                        | `cli-local-transcript.test.ts`    |
| `kernel-parity-export`                | `rust/crates/kernel/tests/parity` (and `test/fixtures/plan-bodies`) | `kernel/tests/replay.rs`, `strict.rs`                                   | `kernel-parity.test.ts`           |
| `kernel-integrate-export`             | `rust/crates/kernel/tests/parity-integrate`                         | `kernel/tests/integrate.rs`                                             | `kernel-integrate-parity.test.ts` |
| `daemon-transcript-export`            | `rust/crates/daemon/tests/transcripts`                              | `daemon/tests/replay.rs`, `cmds.rs`, `budget.rs`, `shadow.rs`           | `daemon-transcript.test.ts`       |
| `loops-parity-export`                 | `rust/crates/daemon/tests/loops-parity`                             | `daemon/tests/loops.rs`                                                 | `loops-parity.test.ts`            |
| `launcher-parity-export`              | `rust/crates/launcher/tests/parity`                                 | `launcher/tests/parity.rs`, `operations.rs`                             | `launcher-parity.test.ts`         |
| `operator-parity-export`              | `rust/crates/operator/tests/parity`                                 | `operator/tests/parity.rs`                                              | `operator-parity.test.ts`         |
| `herdr-parity-export`                 | `rust/crates/herdr/tests/parity`                                    | `herdr/tests/parity.rs`, `adapter_replay.rs`                            | `herdr-parity.test.ts`            |
| `dash-parity-export`                  | `dash/tests/parity`                                                 | `dash/tests/model_parity.rs`, `view_parity.rs`, `e2e_parity.rs`         | `dash-parity.test.ts`             |
| `ledger-parity-export`                | `rust/crates/ledger/tests/parity`                                   | `ledger/tests/migrate.rs`, `refusals.rs`                                | `ledger-rust-interop.test.ts`     |

The first ten rows are the exporters the freeze check ran. Each staleness test builds a fresh export and fails while the
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
`ROUTES` command of `src/daemon.ts` in the operator environment and, where its access allows, the agent environment.
`cli-local-transcript.test.ts` enumerates `ROUTES` and fails on a command with no transcript.

### Normalisation

It follows `test/cli-parity-harness.ts`: a value of a kind is replaced by a placeholder that keeps the kind, and
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
operator and one work item, and the same rows cut back to the first 20, 30 and 36 migrations. They are rebuilt only on
purpose, by `node dist/test/cli-local-transcript-export.js --ledgers`, after which the transcripts are exported again.

### Regenerating

Only for an intended change to a Node behaviour, and see the rules below first:

```
npm run build
node dist/test/cli-transcript-export.js
node dist/test/cli-local-transcript-export.js
```

## After the freeze

1. The freeze point is the main commit the freeze branch started from, and the exporters ran on it. From then the
   committed fixtures are the reference.
2. The version is normalised in the transcripts (`<VERSION>`), so a release bump changes no fixture.
3. Node is frozen, and committed Node fixtures are not edited until plan D (Node removal), so every staleness test keeps
   passing.
4. A deliberate Rust divergence is an overlay: one JSON file per changed case under
   `rust/crates/<crate>/tests/divergences/`, naming the fixture, the expected Rust output and the reason. The schema and
   the reader are in the `capstan-parity-overlay` crate: see `rust/crates/parity-overlay/README.md` (client-lifecycle;
   cli-native finalises the link). The Rust replay reads the overlay in place of the Node expectation, and an overlay
   replaces the whole expected output of a case.
5. The 80 `cstan` transcripts marked `fallback` already hold Node's output under `node`, and cli-native compares against
   that.
6. A Node fix after the freeze re-exports only with PM approval, and no new Node-generated fixtures are made.
7. After Node removal a fixture changes only by hand, in the same commit as the Rust change, with the reason in the commit
   body.
8. Fixtures stay where the Rust tests read them (`rust/crates/*/tests/**`, `test/fixtures/`).
9. The frozen Node tree stays unchanged and keeps passing in the Node CI job.
