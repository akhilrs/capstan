# Install reference

Capstan installs as the `cstan` command. [`install.sh`](../../install.sh) is a POSIX `sh` installer. On Linux x64 and arm64 it downloads the standalone binary of a release (no Node.js needed), checks it against `SHA256SUMS`, and links the command. Where the release has no binary for the machine, or with `--no-binary`, it installs the npm tarball instead (Node 24 and npm). It never reads stdin and never uses `sudo`.

> Releases (binaries, the npm tarball and `SHA256SUMS`) are on the [Releases page](https://github.com/akhilrs/capstan/releases).

## Requirements

| Need                              | Why                                                                                                                                        | Fix if missing                                                                           |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------- |
| Linux or macOS                    | Other systems are not supported                                                                                                            | —                                                                                        |
| Node.js 24 (`>=24.6 <25`) and npm | **Tarball path only** (`--tarball`, `--no-binary`, or a release with no binary for this machine). The binary path needs no Node            | `nvm install 24 && nvm use 24`, or `fnm use 24`                                          |
| `curl` or `wget`                  | Downloads (not needed for a local-path `--binary` or `--tarball`)                                                                          | Install either                                                                           |
| `sha256sum` or `shasum`           | Checksum verification                                                                                                                      | Part of coreutils / macOS                                                                |
| `git`, with at least one commit   | **Hard requirement at run time.** Workers get their own git worktree and branch from HEAD; reports, reviews, plans and integration use git | `git init && git add -A && git commit -m "chore: initial commit"`, or `cstan init --git` |

### The project's git repository

The installer only warns when `git` is missing, but Capstan itself needs the project root to be the top-level folder of a git work tree whose HEAD is a commit. `cstan init` and `cstan start` check this and, when it does not hold, name what is missing and the fix (`git init`, `git add -A`, `git commit -m "chore: initial commit"`). `cstan start` refuses to start the controller, and `cstan spawn` (and so review and replace) fail with the same message. `cstan init --git` does it for you: it runs `git init` only when the folder is not a repository, prints the files it will commit (your `.gitignore` is respected; `.capstan/` is excluded through `.git/info/exclude`) and creates the initial commit. Nothing runs git init or commits without that flag.

- A project root that is a subdirectory of a larger repository is not supported; run `cstan` from the repository's top-level folder.
- A bare repository is refused; use a normal checkout.
- A detached HEAD is supported: workers branch from the commit HEAD points to.
- A repository with no commits is refused until it has one.

Nothing is compiled on either path: there is no `make`, C++ compiler, `python3` or `node-gyp` step. The runtime uses the built-in `node:sqlite`.

`curl` is required when the release base or tarball is a `file://` URL (wget cannot fetch those; the installer says so). With only `wget`, the latest-release lookup reads the `Location` header of the `releases/latest` redirect (`wget --max-redirect=0 -S`); pin `--version` if that fails.

The installer fails with a fix hint for each of these. It only warns when `git`, `herdr` or `claude` are missing, when the bin directory is not on `PATH` (it prints the `export` line), and when another `cstan` earlier on `PATH` shadows the new one.

## Usage

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- <options>
```

### Options and environment variables

| Option                  | Environment            | Meaning                                                                                                                                                                                                 |
| ----------------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--version <x.y.z>`     | `CAPSTAN_VERSION`      | Release to install. Default: the latest, read from the redirect of `https://github.com/akhilrs/capstan/releases/latest` (no API call).                                                                  |
| `--binary <path\|url>`  | `CAPSTAN_BINARY`       | Install this standalone binary instead of a release. Verified against `--sha256`, or against the `SHA256SUMS` file next to it (a `SHA256SUMS` without an entry for the file aborts). For local testing. |
| `--no-binary`           | —                      | Install the npm tarball even when the release has a binary for this machine.                                                                                                                            |
| `--tarball <path\|url>` | `CAPSTAN_TARBALL`      | Install this npm tarball instead of a release. Skips the release lookup. For local testing.                                                                                                             |
| `--sha256 <hex>`        | `CAPSTAN_SHA256`       | Expected checksum. With `--binary` or `--tarball` it can be the only verification; with a release it must also match `SHA256SUMS`.                                                                      |
| `--home <dir>`          | `CAPSTAN_HOME`         | Install root. Default `${XDG_DATA_HOME:-$HOME/.local/share}/capstan`. Must be absolute.                                                                                                                 |
| `--bin-dir <dir>`       | `CAPSTAN_BIN_DIR`      | Where the `cstan` symlink goes. Default `$HOME/.local/bin`. Must be absolute.                                                                                                                           |
| `--uninstall`           | —                      | Remove the install and the bin symlink.                                                                                                                                                                 |
| `--help`                | —                      | Print the options.                                                                                                                                                                                      |
| —                       | `CAPSTAN_RELEASE_BASE` | Replaces `https://github.com/akhilrs/capstan/releases`. For tests.                                                                                                                                      |

Options win over environment variables.

**Which path.** `--binary` installs a binary. `--tarball` or `--no-binary` installs the tarball. Otherwise the installer reads the release's `SHA256SUMS`: if it lists `cstan-<version>-<os>-<arch>` for this machine (`linux-x64` or `linux-arm64`; macOS binaries are not published) it installs that binary, and if not it says so and installs the tarball (older releases have no binaries).

## Layout and upgrades

```text
$CAPSTAN_HOME/current/            the installed build
$CAPSTAN_HOME/current/bin/cstan   the real entry point: the binary itself, or (tarball) the npm entry point
$CAPSTAN_BIN_DIR/cstan            symlink to the entry point above
```

For a binary the installer copies the verified file to `$CAPSTAN_HOME/staging.<pid>/bin/cstan` (mode 755). For the tarball it runs `npm install --global --prefix "$CAPSTAN_HOME/staging.<pid>" --omit=dev --no-audit --no-fund <tarball>`. Either way it runs the staged `cstan --version` and checks it prints `cstan <version>` (and the pinned `--version`, when given). Only then does it swap the staging directory into `current` (`current` → `current.old` → removed). Two consequences:

- The installed path `$CAPSTAN_HOME/current/bin/cstan` is the same across upgrades, so the `.capstan/bin/cstan` wrappers that projects generate keep working.
- A failed install (download, checksum, version check) leaves the previous version untouched. A trap removes the staging directory and temp files on any exit.

Re-running the installer is the upgrade path. If the version is already installed it says so and reinstalls anyway, which also repairs a broken install.

After an upgrade, **running controllers keep the old build** until you run `cstan stop && cstan start`. Lazy imports in a live controller (restart, migrations) can read the new files, so restart promptly.

**Operator restart.** `cstan op propose --restart` works from the standalone binary (see [docs/binary.md](../binary.md#operator-restart)), but the safe way to upgrade after re-running the installer is `cstan stop`, wait until `cstan ping` fails, then `cstan start`.

**Operator restart is unsupported for the npm-tarball install.** `cstan op propose --restart` assumes a repository checkout: it derives `distDir` from the CLI path and renames `dist` and hashes `package.json` under the project root. It is untested against an installed copy; restart a controller with `cstan stop && cstan start` instead.

If `$CAPSTAN_BIN_DIR/cstan` exists and is not a symlink into `$CAPSTAN_HOME`, the installer stops rather than overwrite it.

## Verification

A release download must match the `SHA256SUMS` file published beside the binary or tarball (`sha256sum` or `shasum -a 256`). A mismatch, a missing `SHA256SUMS` or a missing entry aborts before anything is installed. A `--binary` or `--tarball` without `--sha256` (and, for `--binary`, without a `SHA256SUMS` beside it) installs with a warning that it is unverified.

The checksum comes from the same origin as the tarball, so it protects **integrity** (corruption, a truncated or swapped file) and not **authenticity** (a compromised release would carry a matching sum). Releases are not signed. Truncation of the installer script itself is handled differently: all logic is in functions and `main "$@"` is the last line, so a partial download runs nothing.

On the tarball path npm still fetches the runtime dependencies from the registry during the install. The binary path downloads one file.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --uninstall
```

Removes either kind of install: `$CAPSTAN_HOME/current`, any staging directories and the bin symlink, the symlink only when it points into `$CAPSTAN_HOME`. A `cstan` link that points elsewhere is left alone. Running it twice is fine. It never touches your projects' `.capstan/` directories (ledger, operator key, config); delete those yourself. Stop controllers first with `cstan stop`.

## Testing with local files

```sh
npm run release        # writes release/capstan-controller-<version>.tgz, cstan-<version>-linux-{x64,arm64} and release/SHA256SUMS
sh install.sh --binary release/cstan-0.1.1-linux-x64 --home /tmp/cap-home --bin-dir /tmp/cap-bin   # SHA256SUMS beside it verifies it
sh install.sh --tarball release/capstan-controller-0.1.1.tgz --sha256 "$(cut -d' ' -f1 release/SHA256SUMS)" \
  --home /tmp/cap-home --bin-dir /tmp/cap-bin
```

Test a release download without GitHub by serving `download/v<version>/` (the tarball, the binaries and `SHA256SUMS`) and pointing `CAPSTAN_RELEASE_BASE` at it, with a `file://` or `python3 -m http.server` URL.

`sh scripts/test-install.sh` runs the full smoke test against temporary directories: syntax, truncation safety, and for both paths a fresh install, re-install over an existing one, wrong checksum, `--version` pin, uninstall and a tampered download. The binary scenarios run `cstan --version` with no `node` on `PATH`. It builds the release assets first (`npm run release`, which needs network for the Node archives of the binaries) unless `CAPSTAN_TEST_RELEASE_DIR` names a directory that already holds them. It uses `file://` bases, so it needs curl; to cover wget, serve `download/v<version>/` with `python3 -m http.server` and set `CAPSTAN_RELEASE_BASE` to the `http://` URL.

## Troubleshooting

- **"Node 24 is required"**: only on the tarball path. Switch (`nvm use 24`, `fnm use 24`), open a new shell if needed and re-run, or install the standalone binary, which needs no Node.
- **`cstan: command not found`**: add the bin directory to `PATH`, for example `export PATH="$HOME/.local/bin:$PATH"` in your shell profile.
- **Another `cstan` runs instead**: the installer warns when an earlier `PATH` entry shadows it (a leftover `npm link`, for instance). Remove it or reorder `PATH`.
- **Cannot read the latest release**: the `releases/latest` redirect format could change; pin with `--version <x.y.z>`.
- **Controller still runs old code after upgrading**: `cstan stop && cstan start`.

## Bun

Bun is not tested since the native dependencies were removed. `cstan` runs on Node 24 through its shebang, so it still runs on Node when installed that way. The recommended installs are the `curl` one-liner (standalone binary on Linux x64/arm64, needs neither Bun nor Node) or the npm tarball.

History: before the native dependencies (`fs-ext`, `better-sqlite3`) were removed, a plain `bun install -g` gave a broken `cstan` because Bun skipped the native build. That no longer applies.

## Maintainer release steps

The user runs these; the installer and agents do not publish.

```sh
npm run release                                  # tarball, both binaries and SHA256SUMS in release/
git tag v0.1.1 && git push origin main v0.1.1
gh release create v0.1.1 release/capstan-controller-0.1.1.tgz release/cstan-0.1.1-linux-x64 \
  release/cstan-0.1.1-linux-arm64 release/SHA256SUMS --title v0.1.1 --notes "Capstan 0.1.1"
```

`npm run release` builds the binaries (about 130 MB each; the first run downloads the Node archives from nodejs.org) and prints the exact `gh release create` command; it never runs it. The asset names must be `capstan-controller-<version>.tgz` and `cstan-<version>-<os>-<arch>`, with `SHA256SUMS` listing all of them, on tag `v<version>`. Set `CSTAN_RELEASE_DIR` to write somewhere other than `release/`. Then check with `sh scripts/test-install.sh` and a real `curl | sh` into a throwaway `--home`.
