# Install reference

Capstan installs as two static Rust binaries: `cstan` (the command, which is also the controller daemon) and `cstan-dash` (the dashboard). [`install.sh`](../../install.sh) is a POSIX `sh` installer. On Linux x64 and arm64 it downloads the two binaries of a release, checks them against `SHA256SUMS`, and links the command. There is no Node.js, npm or SEA step. It never reads stdin and never uses `sudo`.

> Releases (the four binaries and `SHA256SUMS`) are on the [Releases page](https://github.com/akhilrs/capstan/releases).

> **Until 0.4.0 is released.** The installer on `main` installs only releases from `0.4.0` on (`FIRST_RUST_VERSION` in `install.sh`), and `0.4.0` is the first release cut after the Rust cutover. Until it is published, `install.sh` from `main` installs nothing: it says that the Rust release is not out yet and prints the `install.sh` of the latest published tag (`v0.3.0`) to run instead. The window ends when `0.4.0` is released.

## Requirements

| Need                            | Why                                                                                                                                        | Fix if missing                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------- |
| Linux, x64 or arm64             | The only platforms with published binaries (static musl builds)                                                                            | —                                                                                        |
| `curl` or `wget`                | Downloads (not needed for a local-path `--binary`)                                                                                         | Install either                                                                           |
| `sha256sum` or `shasum`         | Checksum verification                                                                                                                      | Part of coreutils                                                                        |
| `git`, with at least one commit | **Hard requirement at run time.** Workers get their own git worktree and branch from HEAD; reports, reviews, plans and integration use git | `git init && git add -A && git commit -m "chore: initial commit"`, or `cstan init --git` |

Node.js is not needed to install or to run Capstan.

### The project's git repository

The installer only warns when `git` is missing, but Capstan itself needs the project root to be the top-level folder of a git work tree whose HEAD is a commit. `cstan init` and `cstan start` check this and, when it does not hold, name what is missing and the fix (`git init`, `git add -A`, `git commit -m "chore: initial commit"`). `cstan start` refuses to start the controller, and `cstan spawn` (and so review and replace) fail with the same message. `cstan init --git` does it for you: it runs `git init` only when the folder is not a repository, prints the files it will commit (your `.gitignore` is respected; `.capstan/` is excluded through `.git/info/exclude`) and creates the initial commit. Nothing runs git init or commits without that flag.

- A project root that is a subdirectory of a larger repository is not supported; run `cstan` from the repository's top-level folder.
- A bare repository is refused; use a normal checkout.
- A detached HEAD is supported: workers branch from the commit HEAD points to.
- A repository with no commits is refused until it has one.

`curl` is required when the release base is a `file://` URL (wget cannot fetch those; the installer says so). With only `wget`, the latest-release lookup reads the `Location` header of the `releases/latest` redirect (`wget --max-redirect=0 -S`); pin `--version` if that fails.

The installer fails with a fix hint for each of these. It only warns when `git`, `herdr` or `claude` are missing, when the bin directory is not on `PATH` (it prints the `export` line), and when another `cstan` earlier on `PATH` shadows the new one.

## Usage

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- <options>
```

### Options and environment variables

| Option                 | Environment            | Meaning                                                                                                                                                                                                     |
| ---------------------- | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--version <x.y.z>`    | `CAPSTAN_VERSION`      | Release to install. Default: the latest, read from the redirect of `https://github.com/akhilrs/capstan/releases/latest` (no API call). Below `0.4.0` it is refused (see [Older releases](#older-releases)). |
| `--binary <path\|url>` | `CAPSTAN_BINARY`       | Install this `cstan` instead of a release. Verified against `--sha256`, or against the `SHA256SUMS` file next to it (a `SHA256SUMS` without an entry for the file aborts). For local testing.               |
| `--dash-binary <file>` | —                      | Install this `cstan-dash` beside `cstan`. Verified against the `SHA256SUMS` next to it when there is one.                                                                                                   |
| `--no-dash`            | —                      | Do not install `cstan-dash`.                                                                                                                                                                                |
| `--sha256 <hex>`       | `CAPSTAN_SHA256`       | Expected checksum of `cstan`. With `--binary` it can be the only verification; with a release it must also match `SHA256SUMS`.                                                                              |
| `--home <dir>`         | `CAPSTAN_HOME`         | Install root. Default `${XDG_DATA_HOME:-$HOME/.local/share}/capstan`. Must be absolute.                                                                                                                     |
| `--bin-dir <dir>`      | `CAPSTAN_BIN_DIR`      | Where the `cstan` symlink goes. Default `$HOME/.local/bin`. Must be absolute.                                                                                                                               |
| `--uninstall`          | —                      | Remove the install and the bin symlink.                                                                                                                                                                     |
| `--help`               | —                      | Print the options.                                                                                                                                                                                          |
| —                      | `CAPSTAN_RELEASE_BASE` | Replaces `https://github.com/akhilrs/capstan/releases`. For tests.                                                                                                                                          |

Options win over environment variables. `--tarball`, `--no-binary` and `--front-binary` of earlier installers are gone.

**What a release holds.** `cstan-<version>-linux-x64`, `cstan-<version>-linux-arm64`, `cstan-dash-<version>-linux-x64`, `cstan-dash-<version>-linux-arm64` and `SHA256SUMS`. The installer picks the architecture from `uname -m` (`x86_64`/`amd64` is `x64`, `aarch64`/`arm64` is `arm64`; anything else, and any other OS, is refused), downloads both binaries and checks each against `SHA256SUMS`. A release whose `SHA256SUMS` lacks either entry for this machine is refused as incomplete.

### Older releases

`0.4.0` is the first release this installer installs. A release before it (`0.1.1`, `0.2.0`, `0.3.0`) shipped a Node standalone binary or an npm tarball, and its own `install.sh` installs it. Asking for one prints that and the URL to run:

```text
error: Capstan 0.3.0 predates the Rust release 0.4.0, which is the first this installer installs.
Run that tag's own installer instead:
  curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/v0.3.0/install.sh | sh -s -- --version 0.3.0
```

When the **latest** published release is below `0.4.0` the installer says the Rust release is not out yet and names the latest tag's installer the same way. A local `--binary` is not subject to the cutoff.

### The dashboard binary (`cstan-dash`)

`cstan dash` runs `cstan-dash`. There is no Node dashboard any more. Search order, first executable file wins: `CSTAN_DASH_BIN` (absolute path); beside the real path of the running `cstan` (`current/bin/`); `${XDG_DATA_HOME:-$HOME/.local/share}/capstan/current/bin/cstan-dash`; the first `cstan-dash` on `PATH`. A candidate must answer `--version` with `cstan-dash <version>` before it is used. When none is found `cstan dash` prints one line and exits 2. The operator credential reaches it as `CSTAN_DASH_CREDENTIAL` in its environment, readable only by the same user.

## Layout and upgrades

```text
$CAPSTAN_HOME/current/               the installed build
$CAPSTAN_HOME/current/bin/cstan      the command and the controller daemon
$CAPSTAN_HOME/current/bin/cstan-dash the dashboard
$CAPSTAN_BIN_DIR/cstan               symlink to the entry point above
```

The installer copies the verified files to `$CAPSTAN_HOME/staging.<pid>/bin/` (mode 755), runs the staged `cstan --version` and `cstan-dash --version`, and checks they print `cstan <version>` and `cstan-dash <version>` (and the pinned `--version`, when given). Only then does it swap the staging directory into `current` (`current` → `current.old` → removed). Two consequences:

- The installed path `$CAPSTAN_HOME/current/bin/cstan` is the same across upgrades, so the `.capstan/bin/cstan` wrappers that projects generate keep working.
- A failed install (download, checksum, version check) leaves the previous version untouched. A trap removes the staging directory and temp files on any exit.

Re-running the installer is the upgrade path. If the version is already installed it says so and reinstalls anyway, which also repairs a broken install.

### Upgrading from an earlier layout

The swap replaces the whole `current` directory, so whatever an earlier installer wrote is removed. Each of these upgrades in place:

| Earlier install (from)                                  | What was in `current`                                                                                     | After the upgrade                           |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| Node SEA binary (`0.2.0`, `0.3.0`)                      | `bin/cstan` (the Node standalone binary), maybe `bin/cstan-dash`                                          | `bin/cstan` and `bin/cstan-dash`, both Rust |
| Rust front end with `cstan-node` (`0.3.0`)              | `bin/cstan` (the front end), `bin/cstan-node` (the Node binary), `bin/cstan-dash`                         | `bin/cstan-node` is gone                    |
| npm global prefix (the `0.1.1`–`0.3.0` tarball install) | `bin/cstan` → `../lib/node_modules/capstan-controller/…`, the package and its `node_modules` under `lib/` | `lib/` and the npm files are gone           |

The installer says which earlier layout it replaced. It does not stop a daemon. If processes of the old install in `current` are still running, most likely a project's controller daemon, it warns and says to run `cstan stop` and `cstan start` in each project: **running controllers keep the old build until you do.** Wait until `cstan ping` fails after the stop before starting again. Project `.capstan/` directories are never touched, and the ledger migrates on the next start.

If `$CAPSTAN_BIN_DIR/cstan` exists and is not a symlink into `$CAPSTAN_HOME`, the installer stops rather than overwrite it.

## Verification

A release download must match the `SHA256SUMS` file published beside the binaries (`sha256sum` or `shasum -a 256`). A mismatch, a missing `SHA256SUMS` or a missing entry aborts before anything is installed. A `--binary` without `--sha256` (and without a `SHA256SUMS` beside it) installs with a warning that it is unverified.

The checksum comes from the same origin as the binaries, so it protects **integrity** (corruption, a truncated or swapped file) and not **authenticity** (a compromised release would carry a matching sum). Releases are not signed. Truncation of the installer script itself is handled differently: all logic is in functions and `main "$@"` is the last line, so a partial download runs nothing.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --uninstall
```

Removes `$CAPSTAN_HOME/current`, any staging directories and the bin symlink, the symlink only when it points into `$CAPSTAN_HOME`. A `cstan` link that points elsewhere is left alone. Running it twice is fine. It never touches your projects' `.capstan/` directories (ledger, operator key, config); delete those yourself. Stop controllers first with `cstan stop`.

## Testing with local files

`sh scripts/test-install.sh` runs the install scenarios against temporary directories and fabricated releases, with no `node` on `PATH` and no network: syntax, truncation safety, a fresh install, platform detection, checksum refusals, the `0.4.0` cutoff, the upgrade from each earlier layout (fabricated to match what the `0.2.0` and `0.3.0` installers wrote, with and without a running daemon), `--binary`, wget and uninstall. It uses `file://` bases, so it needs `curl`; `latest` is answered by a stub. Set `CAPSTAN_TEST_RELEASE_DIR` to a directory holding a real release (`cstan-<v>-linux-<arch>`, `cstan-dash-<v>-linux-<arch>` and `SHA256SUMS`) to install that too.

To try a release by hand without GitHub, serve a `download/v<version>/` directory with the four binaries and `SHA256SUMS` and point `CAPSTAN_RELEASE_BASE` at it (`file://` or `python3 -m http.server`), or install one binary directly:

```sh
sh install.sh --binary release/cstan-0.4.0-linux-x64 --dash-binary release/cstan-dash-0.4.0-linux-x64 \
  --home /tmp/cap-home --bin-dir /tmp/cap-bin   # the SHA256SUMS beside them verifies them
```

`sh scripts/smoke-binary.sh [cstan binary]` runs the built binaries (no `node` on `PATH`): init, a daemon, status, ping, the agent commands, stop, a Node-made ledger migrating, and `cstan dash` in a pty. See [the release test map](../test-map/release.md).

## Troubleshooting

- **"predates the Rust release" or "is not out yet"**: see [Older releases](#older-releases). Run the install.sh of the tag it names.
- **`cstan: command not found`**: add the bin directory to `PATH`, for example `export PATH="$HOME/.local/bin:$PATH"` in your shell profile.
- **Another `cstan` runs instead**: the installer warns when an earlier `PATH` entry shadows it (a leftover `npm link`, for instance). Remove it or reorder `PATH`.
- **Cannot read the latest release**: the `releases/latest` redirect format could change; pin with `--version <x.y.z>`.
- **Controller still runs old code after upgrading**: `cstan stop && cstan start`.
- **"unsupported OS" or "no Capstan binary for the CPU"**: only Linux x64 and arm64 are published.

## Maintainer release steps

The user runs these; the installer and agents do not publish.

```sh
scripts/release.sh --dry-run          # the next version, the changelog section and the assets the tag will build
scripts/release.sh                    # bump VERSION, package.json, the lockfile and CHANGELOG.md; commit; tag locally
git push origin HEAD v0.4.0           # starts .github/workflows/release.yml
```

`scripts/release.sh` needs `cargo` (it builds `tools/release`); its options are `--dry-run` and `--version X.Y.Z`, and it refuses to run with a dirty tree. It builds no asset. The Release workflow, started by the tag, checks the tag against `VERSION`, builds `cstan` and `cstan-dash` for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` with `cargo-zigbuild`, runs `--version` of each pair on an x64 runner and on the native `ubuntu-24.04-arm` runner, and only then writes `SHA256SUMS` and the GitHub release. `VERSION` is the single version source: `rust/crates/cstan/build.rs` and `dash/build.rs` read it (`CSTAN_VERSION` overrides it; a build outside the repository falls back to the crate version), and `scripts/check-version.sh` fails when `package.json` or the lockfile differ until the npm files go away. `scripts/check-release-workflow.sh` checks the workflow (`--build` also builds both triples locally). Then check with `sh scripts/test-install.sh` and a real `curl | sh` into a throwaway `--home`.
