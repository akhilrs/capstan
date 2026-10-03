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
`capstan-controller-<v>.tgz`, `cstan-<v>-linux-x64`, `cstan-<v>-linux-arm64` and a `SHA256SUMS` listing all three to
`release/` (or `$CSTAN_RELEASE_DIR`). It prints the `gh release create` command and never runs it.
[`install.sh`](../install.sh) installs the binary for the machine when the release's `SHA256SUMS` lists it, checks it with
`<binary> --version`, and links it; no Node is needed. `--no-binary` forces the npm tarball. See
[Install reference](reference/install.md).

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
other's ledgers. arm64 runs only on an arm64 host or with `qemu-aarch64` binfmt; otherwise it is skipped with a
message. Without Herdr, `start` cannot open panes (it exits 4 after the daemon is up), so the launcher does not
write the agent wrapper; the smoke test then runs a wrapper of the same one-line shape, and the launcher
unit test (`test/sea.test.ts`) covers the generated script.

## macOS (not built by default)

`--target darwin-arm64|darwin-x64` works on a Mac only: the build runs `codesign --remove-signature` before
`postject` (with `--macho-segment-name NODE_SEA`) and an ad-hoc `codesign --sign -` afterwards. That is enough to
run locally. A binary you distribute needs a Developer ID signature and Apple notarization, which this repo does
not do.
