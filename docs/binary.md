# The release binaries

A release holds two static Rust programs per platform (Linux x64 and arm64, static musl; macOS arm64 and x64, built for macOS 11 and later and built and checked on GitHub's macOS 15 runners):

- `cstan`: the command and the controller daemon (`cstan daemon`). Source in `rust/crates/cstan`, built from the
  `cstan-front` package. It needs no runtime or compiler on the machine.
- `cstan-dash`: the dashboard. Source in `dash/`. `cstan dash` replaces itself with it.

The assets are `cstan-<version>-linux-{x64,arm64}`, `cstan-<version>-macos-{arm64,x64}`, `cstan-dash-<version>-linux-{x64,arm64}`, `cstan-dash-<version>-macos-{arm64,x64}` and `SHA256SUMS`, for example `cstan-<version>-macos-arm64` and `cstan-dash-<version>-macos-arm64`.
[`install.sh`](../install.sh) installs them as `current/bin/cstan` and `current/bin/cstan-dash`; see the
[Install reference](reference/install.md).

## How they are built

`VERSION` at the repository root is the single version source. `rust/crates/cstan/build.rs` and `dash/build.rs` embed it
(`CSTAN_VERSION` in the environment overrides it; a build outside the repository falls back to the crate's own version).

The Release workflow (`.github/workflows/release.yml`, started by a `v*` tag that `scripts/release.sh` creates locally)
builds the four Linux binaries with `cargo zigbuild --release --locked` for `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`. cargo-zigbuild supplies the C cross toolchain that `capstan-ledger` (bundled SQLite)
needs. An x64 job and a job on the native `ubuntu-24.04-arm` runner each run `cstan --version` and
`cstan-dash --version` of their architecture against the tag, and only then does the publish job write `SHA256SUMS` and the
GitHub release.

The macOS binaries are built natively, not cross-built: a `build-macos` job runs `cargo build --release --locked` for `aarch64-apple-darwin` on `macos-15` and for `x86_64-apple-darwin` on `macos-15-intel`, with `MACOSX_DEPLOYMENT_TARGET=11.0` so they run on macOS 11 and later. Each binary is ad-hoc signed (`codesign --force --sign -`), checked with `codesign --verify --strict` and `lipo -archs`, and run with `--version` before publish writes the eight assets into `SHA256SUMS`. They are not notarized: `curl | sh` installs them without a prompt because `install.sh` clears `com.apple.quarantine`, and a binary downloaded by hand needs `xattr -d com.apple.quarantine <file>` (see the [Install reference](reference/install.md)). The workflow also runs by hand (`workflow_dispatch`) on any branch as a dry run: it builds and checks everything and skips publish. See [DEC-008](../decisions/DEC-008-macos-release.md).

A Linux developer checks the macOS build with `scripts/check-darwin.sh` (clippy for `<arch>-apple-darwin` with zig cc). CI's `macos` job on `macos-15` runs `scripts/check.sh unit`, an `x86_64-apple-darwin` cargo check and `scripts/test-install.sh`. `scripts/smoke-binary.sh`, `scripts/daemon-survivors.sh` and the integration tests are Linux-only. Known limit: PM-359 ([DEC-007](../decisions/DEC-007-macos-process-information.md)).

To build the host binaries from a checkout (needs Rust, see `rust/rust-toolchain.toml`):

```sh
(cd rust && cargo build --release --locked -p cstan-front)   # rust/target/release/cstan
(cd dash && cargo build --release --locked)                  # dash/target/release/cstan-dash
```

For the musl targets locally: `cargo install cargo-zigbuild --locked`, `zig` from https://ziglang.org/download,
`rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`, then
`sh scripts/check-release-workflow.sh --build` (set `CARGO_TARGET_DIR` outside the checkout). It builds both triples,
checks each with `file` and runs the arm64 pair under `qemu-aarch64` when that is installed.

## The dashboard (`cstan-dash`)

`cstan dash` starts the daemon, finds `cstan-dash` and replaces itself with it. There is no other dashboard.

- **Search order**, first executable file wins: `CSTAN_DASH_BIN` (absolute path); beside the real path of the running
  `cstan` (`current/bin/`); `${XDG_DATA_HOME:-$HOME/.local/share}/capstan/current/bin/cstan-dash`; the first `cstan-dash`
  on `PATH`.
- **Probe.** A candidate is used only after `<bin> --version` (5 s limit) prints `cstan-dash <version>`.
- **None found.** One line on stderr (`cstan-dash was not found; install it beside cstan ...`) and exit 2.
- **Hand-over.** The dashboard gets `--socket`, `--interval`, `--worker-limit` (when known), `--no-color` and
  `--reduced-motion`. The environment loses `CAPSTAN_TOKEN` and `CAPSTAN_SOCKET` and gains `CSTAN_DASH_CREDENTIAL`, the
  operator credential, readable by the same user through `/proc` (the key file is readable by that user anyway).

## Operator restart

`cstan op propose --restart` works from the installed binary. After the controller has answered requests for the settle
time with an unchanged binary it saves a copy as `.capstan/state/known-good/cstan` with a manifest. The helper is
`<known-good binary> __restart-helper <plan>`: it runs from the saved copy, waits for the old controller to go, backs up
the ledger, starts the file at the original path with `daemon` and pings it. A new binary that does not answer ping is
renamed to `cstan.failed-<proposal>` and the known-good copy is renamed into its place.

To replace the binary for a restart, put the new file in place with a rename (`mv new cstan`, or run `install.sh`, which
does). Do not `cp` over a running binary: Linux refuses with `Text file busy`.

## Upgrading by hand (safe order)

```sh
cstan stop
until ! cstan ping >/dev/null 2>&1; do sleep 1; done   # wait until the controller is really gone
sh install.sh                                           # or: mv cstan.new "$(command -v cstan)"
cstan start
```

Wait for `cstan ping` to fail before replacing or starting anything: `cstan stop` returns when the stop is requested,
and a second controller must not start while the first still holds the ledger. Running controllers keep the old build
until they are restarted.

## Smoke test

`sh scripts/smoke-binary.sh [cstan binary...]` (part of `scripts/check.sh`) runs each binary in a temporary HOME and git
repository with `PATH` limited to the binary's directory plus `/usr/bin:/bin`. It runs
`scripts/check-version.sh`, then checks `--version`, `--help`, `init`, `cstan daemon` started with `CAPSTAN_LAUNCH=off`
(its `/proc/<pid>/exe` is the binary), `ping`, `status`, `inbox --hook`, empty stderr and `stop`. It then opens a ledger made
by an earlier release (`rust/testdata/ledger/ledger-better-sqlite3.sqlite`, migration 31), checks that it migrates to the current migration with
the same `schema_migrations` rows and checksums and a backup, and runs send, inbox and ack as the agents in it. Last, with a
`cstan-dash` for the same architecture (`CSTAN_DASH_SMOKE_BIN`, `release/cstan-dash-<v>-<platform>` or a host build), it
runs `cstan dash` in a real terminal (a pty from `script`), checks the process became `cstan-dash`, sees its first frame and
ends it with `q`. arm64 runs only on an arm64 host or with `qemu-aarch64` binfmt; otherwise it is skipped with a message
(the Release workflow runs the arm64 binaries on a native runner).
