# Install reference

Capstan installs as the `cstan` command. [`install.sh`](../../install.sh) is a POSIX `sh` installer that downloads a release tarball, checks it, installs it with npm and links the command. It never reads stdin and never uses `sudo`.

> Until a release (`v0.1.1`) is published on GitHub and `main` is pushed, only the local-tarball path works. The release download and the `curl | sh` URL need both.

## Requirements

| Need                                                          | Why                                                        | Fix if missing                                                                             |
| ------------------------------------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Linux or macOS                                                | Other systems are not supported                            | —                                                                                          |
| Node.js 24 (`>=24 <25`) and npm                               | The `engines` range; `fs-ext` is compiled for this Node    | `nvm install 24 && nvm use 24`, or `fnm use 24`                                            |
| `make`, a C++ compiler (`c++`, `g++` or `clang++`), `python3` | node-gyp builds the native `fs-ext` module at install time | Debian/Ubuntu: `sudo apt install build-essential python3`; macOS: `xcode-select --install` |
| `curl` or `wget`                                              | Downloads (not needed for a local-path `--tarball`)        | Install either                                                                             |
| `sha256sum` or `shasum`                                       | Checksum verification                                      | Part of coreutils / macOS                                                                  |

`curl` is required when the release base or tarball is a `file://` URL (wget cannot fetch those; the installer says so). With only `wget`, the latest-release lookup reads the `Location` header of the `releases/latest` redirect (`wget --max-redirect=0 -S`); pin `--version` if that fails.

The native `fs-ext` build runs node-gyp, which downloads Node headers from nodejs.org unless they are cached in `~/.cache/node-gyp` or `npm_config_nodedir` points at a Node source/headers directory, and which needs a `python3` version node-gyp supports. An offline install therefore needs both a warm npm cache and cached headers.

The installer fails with a fix hint for each of these. It only warns when `git`, `herdr` or `claude` are missing, when the bin directory is not on `PATH` (it prints the `export` line), and when another `cstan` earlier on `PATH` shadows the new one.

## Usage

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- <options>
```

### Options and environment variables

| Option                  | Environment            | Meaning                                                                                                                                |
| ----------------------- | ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `--version <x.y.z>`     | `CAPSTAN_VERSION`      | Release to install. Default: the latest, read from the redirect of `https://github.com/akhilrs/capstan/releases/latest` (no API call). |
| `--tarball <path\|url>` | `CAPSTAN_TARBALL`      | Install this tarball instead of a release. Skips the release lookup. For local testing.                                                |
| `--sha256 <hex>`        | `CAPSTAN_SHA256`       | Expected checksum. With `--tarball` it is the only verification; with a release it must also match `SHA256SUMS`.                       |
| `--home <dir>`          | `CAPSTAN_HOME`         | Install root. Default `${XDG_DATA_HOME:-$HOME/.local/share}/capstan`. Must be absolute.                                                |
| `--bin-dir <dir>`       | `CAPSTAN_BIN_DIR`      | Where the `cstan` symlink goes. Default `$HOME/.local/bin`. Must be absolute.                                                          |
| `--uninstall`           | —                      | Remove the install and the bin symlink.                                                                                                |
| `--help`                | —                      | Print the options.                                                                                                                     |
| —                       | `CAPSTAN_RELEASE_BASE` | Replaces `https://github.com/akhilrs/capstan/releases`. For tests.                                                                     |

Options win over environment variables.

## Layout and upgrades

```text
$CAPSTAN_HOME/current/            npm global prefix of the installed build
$CAPSTAN_HOME/current/bin/cstan   the real entry point
$CAPSTAN_BIN_DIR/cstan            symlink to the entry point above
```

The installer runs `npm install --global --prefix "$CAPSTAN_HOME/staging.<pid>" --omit=dev --no-audit --no-fund <tarball>`, runs the staged `cstan --version` and checks it prints `cstan <version>`. Only then does it swap the staging directory into `current` (`current` → `current.old` → removed). Two consequences:

- The installed path `$CAPSTAN_HOME/current/bin/cstan` is the same across upgrades, so the `.capstan/bin/cstan` wrappers that projects generate keep working.
- A failed install (download, checksum, compile, version check) leaves the previous version untouched. A trap removes the staging directory and temp files on any exit.

Re-running the installer is the upgrade path. If the version is already installed it says so and reinstalls anyway, which also repairs a broken install.

After an upgrade, **running controllers keep the old build** until you run `cstan stop && cstan start`. Lazy imports in a live controller (restart, migrations) can read the new files, so restart promptly.

If you switch Node major version, re-run the installer: `fs-ext` is compiled for the Node that ran the install.

**Operator restart is unsupported for installed copies.** `cstan op propose --restart` assumes a repository checkout: it derives `distDir` from the CLI path and renames `dist` and hashes `package.json` under the project root. It is untested against an installed copy; restart a controller with `cstan stop && cstan start` instead.

If `$CAPSTAN_BIN_DIR/cstan` exists and is not a symlink into `$CAPSTAN_HOME`, the installer stops rather than overwrite it.

## Verification

A release download must match the `SHA256SUMS` file published beside the tarball (`sha256sum` or `shasum -a 256`). A mismatch, a missing `SHA256SUMS` or a missing entry aborts before anything is installed. A `--tarball` without `--sha256` installs with a warning that it is unverified.

The checksum comes from the same origin as the tarball, so it protects **integrity** (corruption, a truncated or swapped file) and not **authenticity** (a compromised release would carry a matching sum). Releases are not signed. Truncation of the installer script itself is handled differently: all logic is in functions and `main "$@"` is the last line, so a partial download runs nothing.

npm still fetches the runtime dependencies from the registry during the install.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --uninstall
```

Removes `$CAPSTAN_HOME/current`, any staging directories and the bin symlink, the symlink only when it points into `$CAPSTAN_HOME`. A `cstan` link that points elsewhere is left alone. Running it twice is fine. It never touches your projects' `.capstan/` directories (ledger, operator key, config); delete those yourself. Stop controllers first with `cstan stop`.

## Testing with a local tarball

```sh
npm run release        # writes release/capstan-controller-<version>.tgz and release/SHA256SUMS
sh install.sh --tarball release/capstan-controller-0.1.1.tgz --sha256 "$(cut -d' ' -f1 release/SHA256SUMS)" \
  --home /tmp/cap-home --bin-dir /tmp/cap-bin
```

Test a release download without GitHub by serving `download/v<version>/` (the tarball and `SHA256SUMS`) and pointing `CAPSTAN_RELEASE_BASE` at it, with a `file://` or `python3 -m http.server` URL.

`sh scripts/test-install.sh` runs the full smoke test against temporary directories: syntax, truncation safety, fresh install, re-run, wrong checksum, wrong Node, uninstall and release mode with a tampered tarball. It needs no network beyond npm's dependency fetch, but a warm npm cache alone is not enough offline: node-gyp also needs cached Node headers (`~/.cache/node-gyp`, or `npm_config_nodedir`) and a supported `python3`. The test passes your existing npm cache and node-gyp dir through to its temp HOMEs. It uses `file://` bases, so it needs curl; to cover wget, serve `download/v<version>/` with `python3 -m http.server` and set `CAPSTAN_RELEASE_BASE` to the `http://` URL. Set `CAPSTAN_TEST_RELEASE_DIR` to a directory containing the tarball and `SHA256SUMS` to skip `npm run release`.

## Troubleshooting

- **"Node 24 is required"**: switch (`nvm use 24`, `fnm use 24`), open a new shell if needed and re-run. After any later Node major change, re-run the installer.
- **Compile errors from `fs-ext`**: install the toolchain (`build-essential python3` on Debian/Ubuntu, `xcode-select --install` on macOS). node-gyp also downloads Node headers on first use, which needs network access.
- **`cstan: command not found`**: add the bin directory to `PATH`, for example `export PATH="$HOME/.local/bin:$PATH"` in your shell profile.
- **Another `cstan` runs instead**: the installer warns when an earlier `PATH` entry shadows it (a leftover `npm link`, for instance). Remove it or reorder `PATH`.
- **Cannot read the latest release**: the `releases/latest` redirect format could change; pin with `--version <x.y.z>`.
- **Controller still runs old code after upgrading**: `cstan stop && cstan start`.

## Bun

Not supported yet. A plain `bun install -g` gives a broken `cstan` today.

Evidence, with Bun 1.4.2:

- `bun install -g <tgz>` reports success, but `cstan --version` then fails with `Cannot find module ./build/Release/fs_ext.node`. Bun skips the native install scripts of untrusted dependencies, so `fs-ext` is never compiled.
- When `bun` is not on `PATH`, the install script of `better-sqlite3` also fails (exit 127).

`cstan` always runs on Node 24 through its shebang, even when Bun installed it, so Bun could only ever be the installer, never the runtime. Whether a trusted install (`bun pm -g trust`) works is untested.

Follow-up: replacing `fs-ext` (used for `flock`) would remove that native build.

Use `install.sh` or npm instead.

## Maintainer release steps

The user runs these; the installer and agents do not publish.

```sh
npm run release                                  # build the tarball and SHA256SUMS in release/
git tag v0.1.1 && git push origin main v0.1.1
gh release create v0.1.1 release/capstan-controller-0.1.1.tgz release/SHA256SUMS \
  --title v0.1.1 --notes "Capstan 0.1.1"
```

The asset name must be `capstan-controller-<version>.tgz`, with `SHA256SUMS` on tag `v<version>`. Then check with `sh scripts/test-install.sh` and a real `curl | sh` into a throwaway `--home`.
