# Standalone binary

`npm run build:binary` builds `release/cstan-<version>-linux-x64` and `release/cstan-<version>-linux-arm64`:
one executable that runs `cstan` with no Node.js, npm or compiler on the machine. It is a Node.js
[single-executable application](https://nodejs.org/api/single-executable-applications.html) (SEA).
`npm run build:binary -- --target linux-x64` builds one target. The binary is about 130 MB because it is
the whole Node.js runtime plus the CLI.

Building needs the Node.js you run the build with (24.6 or newer, the same version is used for the
runtime), network access to nodejs.org, and `tar`/`xz`. Nothing is compiled: the dev dependencies
`esbuild` and `postject` ship prebuilt binaries or WebAssembly. `npm ci` runs no `node-gyp`; the
runtime uses the built-in `node:sqlite`, so `better-sqlite3` and `fs-ext` are gone.

## How it is built

1. **Bundle.** esbuild bundles `src/cli.ts` with ink, react and yoga into one ESM file. `react-devtools-core`,
   an optional ink peer that only loads under `DEV=true`, is stubbed. `import.meta.url` is shimmed to the
   binary's path, and the package version is a build-time define (`__CAPSTAN_VERSION__`).
2. **Why ESM, not CommonJS.** A SEA main script must be CommonJS on Node 24. A `format=cjs` build fails:
   `yoga-layout` (`await loadYoga()`) and ink's `devtools.js` use top-level `await`, which CommonJS cannot
   express. So the ESM bundle is stored as the SEA asset `bundle.mjs` and the CommonJS main is a ~40 line
   stub. On start the stub writes the asset once to
   `$XDG_CACHE_HOME/capstan/sea/<sha256 of the bundle>/cstan.mjs` (default `~/.cache/capstan`). The directories
   are mode 0700 and must be real directories owned by the user (a symlink or a foreign owner is refused); the
   file is written to a mode 0600 temp file in the same directory, fsynced and renamed into place. Before every
   start the cached file is compared with the embedded bytes and rewritten on any difference, so stale or
   tampered content never runs. If the directory cannot be written, the binary exits with a message naming it;
   it never falls back to `/tmp`. The directory name is the content hash, so a new binary never runs an older
   bundle. Old hash directories are kept: nothing prunes them (each is about 2.4 MB), so delete
   `~/.cache/capstan/sea` to reclaim them. The bundle is self-contained (builtins only), so the
   import never resolves into `node_modules`. The first run needs a writable cache directory.
3. **Assets.** Every `migrations/*.sql` is embedded as `migrations/<name>`. In the binary `readMigration`
   reads those bytes, so migration checksums equal those of the files on disk: a ledger made by the npm
   build opens from the binary and the other way round (the smoke test checks both).
4. **Blob and injection.** `sea-config.json` sets `useCodeCache` and `useSnapshot` to false, so the blob
   does not depend on the CPU architecture, and `disableExperimentalSEAWarning`. For each target the build
   downloads `node-<process.version>-linux-<arch>.tar.xz` from nodejs.org, checks it against
   `SHASUMS256.txt`, copies out the `node` binary (cached in `release/node-cache`, which can be deleted),
   and injects the blob with `postject` and the `NODE_SEA_FUSE` sentinel. The blob is built once and is
   identical for both architectures.

## Behaviour that differs from the npm install

- `process.argv` is `[binary, binary, ...args]` on Node 24.6; the CLI reads `argv.slice(2)` either way.
  Node's own flags (`--inspect`, `-e`) are not interpreted; they reach `cstan` as arguments.
- Every self-invocation goes through `src/sea.ts` (`isSea`, `selfCommand`, `entryPath`, `readAsset`).
  The daemon is started as `<binary> daemon`, and the per-agent `cstan` wrapper and the status pane run
  `exec '<binary>' ...`. They name no Node and no `cli.js`. The PostToolUse hook still runs `cstan inbox --hook`
  resolved on the agents' PATH.
- **Operator restart replaces the binary file.** See below.

## Releases and install

`npm run release` packs the npm tarball, builds both binaries (`npm run build:binary`) and writes
`capstan-controller-<v>.tgz`, `cstan-<v>-linux-x64`, `cstan-<v>-linux-arm64`, the two `cstan-front-<v>-linux-<arch>` files, the two `cstan-dash-<v>-linux-<arch>` files and a `SHA256SUMS` listing all of them to
`release/` (or `$CSTAN_RELEASE_DIR`). It prints the `gh release create` command and never runs it.
[`install.sh`](../install.sh) installs the binary for the machine when the release's `SHA256SUMS` lists it, checks it with
`<binary> --version`, and links it; no Node is needed. With `cstan-front` in the release the layout is the one under "The native front end". `--no-binary` forces the npm tarball. See
[Install reference](reference/install.md).

## The native front end (`cstan` beside `cstan-node`)

`cstan-front` (source in `rust/crates/cstan`) is a small static Rust program, about 1 MB, that runs the commands an agent
sends over the operator socket (`ping`, `send`, `inbox`, `ack`, `wait`, ...) without starting the 130 MB Node runtime, and
hands every other command to the Node binary unchanged.

- **Layout.** A binary install holds `current/bin/cstan` (the front end), `current/bin/cstan-node` (the SEA, the release asset
  `cstan-<v>-<platform>`; only the installed file name changes) and, when present, `current/bin/cstan-dash`. The `cstan`
  symlink in the bin directory points at `current/bin/cstan`. Without a front end the SEA is `current/bin/cstan`, as in 0.2.0.
- **Hand-over.** The front end replaces itself with the Node implementation: `CSTAN_NODE_CLI` first (an absolute path; a
  `.js` or `.mjs` file runs under `CSTAN_NODE` or `node`, anything else runs as it is), then `cstan-node` beside the front end.
  A candidate that is the front end itself is skipped, so the hand-over cannot loop.
- **Agents.** The per-agent `cstan` wrapper the daemon writes starts the front end and sets `CSTAN_NODE_CLI` to the SEA, or
  to `cli.js` with `CSTAN_NODE` for the npm build. Under SEA the front end is `CSTAN_FRONT_END` when that names an
  absolute executable file, else a `cstan` beside the binary that is not the binary itself. With none, the wrapper runs the Node CLI as before.
- **Build.** `npm run build:cli` builds the host front end into `rust/target/release/cstan`;
  `npm run build:cli -- --target linux-x64` (or `linux-arm64`) builds the musl binary into
  `release/cstan-front-<version>-<target>`. `npm run release` builds both targets, lists them in `SHA256SUMS` and in the
  `gh release create` command; `--no-front` skips them and a release without `cargo` fails unless `--no-front` is given.
- **Install.** `install.sh` checks `cstan-front-<v>-<platform>` against `SHA256SUMS`, stages the SEA as `bin/cstan-node` and
  the front end as `bin/cstan`, and keeps them only if `bin/cstan __front-version` prints `cstan-front <this release>`.
  This is the **fallback rule**: a front end that does not run on this machine, or belongs to another release, is dropped
  with a warning and the SEA stays `bin/cstan`; a release without the asset installs the SEA alone. `--front-binary <file>`
  installs a file you have (checked against a `SHA256SUMS` beside it) and `--no-front` skips it. Uninstall removes the whole
  `current` directory, both files included. Tarball installs have no front end.
- **Old installers.** The asset names are unchanged, so an `install.sh` from 0.2.0 against a release with a front end
  installs the SEA as `bin/cstan` and works as before.
- **Operator restart** still replaces the file the coordinator runs from: the daemon is `cstan-node daemon`, and the restart
  swaps `cstan-node`, not the front end.
- **Checks without cargo.** `npm run check` runs `check:dash` (rustfmt, clippy and the tests of `dash/` and `rust/`, and the host
  front end build the parity tests use). A machine without cargo sets **both** `CSTAN_SKIP_DASH_CHECK=1` (the Rust checks print
  a loud `SKIPPED`) and `CSTAN_SKIP_FRONT_PARITY=1` (the tests that compare the front end with the Node CLI are skipped).

## The Rust dashboard (`cstan-dash`)

`cstan dash` runs a Rust program, `cstan-dash` (source in `dash/`), when it can find one, and the Node dashboard
otherwise. It is a separate small static binary, not part of the SEA.

- **Build.** `npm run build:dash` runs `cargo build --release --locked` in `dash/` and leaves
  `dash/target/release/cstan-dash`, where a source checkout finds it. `npm run build:dash -- --target linux-x64`
  (or `linux-arm64`, repeatable) builds the static musl binary and writes `release/cstan-dash-<version>-linux-<arch>`.
  The musl targets need `rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`;
  `dash/.cargo/config.toml` links aarch64 with `rust-lld`, so no cross gcc is needed.
- **Release.** `npm run release` builds both cstan-dash targets after the cstan binaries and lists them in `SHA256SUMS`
  and in the `gh release create` command. `--no-dash` skips them; without `cargo` the release fails unless
  `--no-dash` is given. `--dry-run` lists every asset.
- **Install.** `install.sh` installs `cstan-dash` beside `cstan` (`current/bin/`) when `SHA256SUMS` lists it, after
  checking it the same way as the binary. A release without it installs `cstan` alone and says so.
- **Launch.** Before loading react or ink, `runDash` searches in this order and takes the first executable file:
  `CSTAN_DASH_BIN` (absolute path); beside the realpath of the running `cstan` (the SEA binary and the installer's
  `current/bin/`); `<repo>/dash/target/release/cstan-dash` when running from `dist/src/cli.js`;
  `${XDG_DATA_HOME:-$HOME/.local/share}/capstan/current/bin/cstan-dash`; the first `cstan-dash` on `PATH`.
  `CSTAN_DASH=node` skips the search and runs the Node dashboard; `CSTAN_DASH=rust` fails when none is found.
- **Probe.** A candidate is used only after `<bin> --version` (2 s limit) prints `cstan-dash <version>`; a file that
  does not run is skipped for the next one, and `CSTAN_DASH=rust` then fails naming it.
- **Hand-over.** The dashboard replaces the `cstan` process with `process.execve` (experimental in Node 24, so no
  Node process stays resident) with `--socket`, `--interval`, `--worker-limit` (when known), `--no-color` and
  `--reduced-motion`. The environment loses `CAPSTAN_TOKEN` and `CAPSTAN_SOCKET` and gains `CSTAN_DASH_CREDENTIAL`.
  `execve` aborts the process when the exec fails, so it is only used for an ELF file or a script with an executable
  interpreter; anything else is run as a child with inherited stdio, `SIGTERM`/`SIGHUP` forwarded and the child's
  exit code returned. A candidate that cannot be started (ENOENT, EACCES, ENOEXEC) falls back to the Node dashboard.
- **Fallback.** With no binary, `cstan dash` runs the Node dashboard and prints one line after it exits:
  `cstan: using the Node dashboard; install cstan-dash for lower CPU and memory: ...`.
- **Credential.** The operator credential is in the dashboard's environment, readable by the same user through
  `/proc`; the key file is readable by that user anyway.

## Operator restart

`cstan op propose --restart` works from the binary. The controller treats its own executable the way the npm
build treats `dist`:

- **Known-good snapshot.** After the controller has answered requests for the settle time (60 s) with an unchanged
  binary (sha256 of the file, taken at start and again before the copy), it saves a copy as
  `.capstan/state/known-good/cstan` (mode 0755) with a manifest (version, highest migration). There are no dependency
  files, so the "dependencies changed" warning never applies. A restart refuses with `no_known_good` until the copy
  exists.
- **The helper** is `<known-good binary> __restart-helper <plan>`, a hidden subcommand of the CLI. It runs from the
  saved copy, not from the file being replaced, so a bad new binary cannot break the helper. It waits for the old
  controller to be gone, backs up the ledger, starts the file at the original path with `daemon`, and pings it.
- **Rollback.** A new binary that does not answer ping is stopped; the file is renamed to
  `cstan.failed-<proposal>` and a copy of the known-good binary, written next to it as a temp file and then
  renamed over the path, takes its place. Every swap is a rename, and the helper never writes into a file that is
  running, so a running executable is never truncated. The ledger backup is restored when the stored schema is
  newer than the known-good binary knows, as for the npm build.
- **How to replace the binary for a restart.** Put the new file in place with a rename (`mv new cstan`, or run
  `install.sh`, which does). Do not `cp` over a running binary: Linux refuses with `Text file busy`. The coordinator
  starts whatever file is at the controller's own path.

`test/operator-restart-sea.test.ts` runs the real binary: the coordinator takes the snapshot, the helper starts a
replacement, the replacement serves and the old pid is gone, and a replacement that exits is rolled back. It needs
`release/cstan-<version>-linux-x64` (or `CSTAN_TEST_BINARY`) and is skipped without one.

## Upgrading by hand (safe order)

If you would rather not use `op propose --restart`, for example with several projects sharing one binary:

```sh
cstan stop
until ! cstan ping >/dev/null 2>&1; do sleep 1; done   # wait until the controller is really gone
sh install.sh                                           # or: mv cstan.new "$(command -v cstan)"
cstan start
```

Wait for `cstan ping` to fail before replacing or starting anything: `cstan stop` returns when the stop is
requested, and a second controller must not start while the first still holds the ledger. A replaced binary
starts as soon as `cstan start` runs; running controllers keep the old build until then.

## Smoke test

`npm run smoke:binary` (or `sh scripts/smoke-binary.sh [binary...]`) runs each binary in a temporary HOME
and git repo with `PATH` limited to the binary's directory plus `/usr/bin:/bin` and no node. It checks
`--version`, `--help`, `init`, `start` (the daemon's `/proc/<pid>/exe` is the binary), `status`,
`inbox --hook` (silent, exit 0), the agent wrapper, one `dash` frame in a pty, empty stderr, `stop` and that the
daemon pid is gone. When `node` and `dist/` exist it also checks that the npm build and the binary open each
other's ledgers. When a front end is available (`release/cstan-front-<v>-<platform>`, the host build of `npm run build:cli`
for x64, or `CSTAN_FRONT_SMOKE_BIN`) it repeats the run in the installed layout (`cstan` + `cstan-node`): `--version`,
`init`, `start` (the daemon's `/proc/<pid>/exe` is `cstan-node`), agent-environment `ping`, `status`, `send`, `inbox`
and `ack` with the operator key (without Herdr there is no agent token, so the daemon refuses some; the front end's stdout,
stderr and exit code must equal `cstan-node`'s), a held agent `wait` (its `/proc/<pid>/exe` is the front end), the
launcher's wrapper script, and `cstan dash` becoming `cstan-dash`. arm64 runs only on an arm64 host or with `qemu-aarch64` binfmt; otherwise it is skipped with a
message. Without Herdr, `start` cannot open panes (it exits 4 after the daemon is up), so the launcher does not
write the agent wrapper; the smoke test then runs a wrapper of the same one-line shape, and the launcher
unit test (`test/sea.test.ts`) covers the generated script.

## macOS (not built by default)

`--target darwin-arm64|darwin-x64` works on a Mac only: the build runs `codesign --remove-signature` before
`postject` (with `--macho-segment-name NODE_SEA`) and an ad-hoc `codesign --sign -` afterwards. That is enough to
run locally. A binary you distribute needs a Developer ID signature and Apple notarization, which this repo does
not do.
