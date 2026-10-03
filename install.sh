#!/bin/sh
# Capstan installer.
#
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --version 0.1.1
#
# POSIX sh. Everything lives in functions and `main "$@"` is the last line, so a
# truncated download defines functions and runs nothing. Never reads stdin,
# never uses sudo. Run with --help for the options.
set -eu

REPO_URL="https://github.com/akhilrs/capstan"
NODE_MAJOR_REQUIRED=24

STAGING=""
TMP_DIR=""

say() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Install the Capstan controller (the `cstan` command).

Usage: install.sh [options]

Options:
  --version <x.y.z>     Release to install (env CAPSTAN_VERSION). Default: the latest release.
  --tarball <path|url>  Install this tarball instead of a release (env CAPSTAN_TARBALL).
  --sha256 <hex>        Expected sha256 of the tarball (env CAPSTAN_SHA256).
  --home <dir>          Install root (env CAPSTAN_HOME).
                        Default: ${XDG_DATA_HOME:-$HOME/.local/share}/capstan
  --bin-dir <dir>       Where the cstan symlink goes (env CAPSTAN_BIN_DIR).
                        Default: $HOME/.local/bin
  --uninstall           Remove the install and the bin symlink.
  --help                Show this help.

Environment: CAPSTAN_RELEASE_BASE overrides https://github.com/akhilrs/capstan/releases (tests).
Re-running the installer upgrades. Project .capstan/ directories are never touched.
EOF
}

# True only for digits.digits.digits with nothing before or after.
valid_version() {
  case "$1" in
    "" | *[!0-9.]* | .* | *. | *..*) return 1 ;;
  esac
  [ "$(printf '%s' "$1" | tr -cd . | wc -c | tr -d ' ')" = 2 ]
}

parse_args() {
  VERSION="${CAPSTAN_VERSION:-}"
  TARBALL="${CAPSTAN_TARBALL:-}"
  SHA256="${CAPSTAN_SHA256:-}"
  HOME_DIR="${CAPSTAN_HOME:-}"
  BIN_DIR="${CAPSTAN_BIN_DIR:-}"
  RELEASE_BASE="${CAPSTAN_RELEASE_BASE:-$REPO_URL/releases}"
  UNINSTALL=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --version | --tarball | --sha256 | --home | --bin-dir)
        [ $# -ge 2 ] || die "$1 needs a value"
        case "$1" in
          --version) VERSION="$2" ;;
          --tarball) TARBALL="$2" ;;
          --sha256) SHA256="$2" ;;
          --home) HOME_DIR="$2" ;;
          --bin-dir) BIN_DIR="$2" ;;
        esac
        shift 2
        ;;
      --version=* | --tarball=* | --sha256=* | --home=* | --bin-dir=*)
        value="${1#*=}"
        case "$1" in
          --version=*) VERSION="$value" ;;
          --tarball=*) TARBALL="$value" ;;
          --sha256=*) SHA256="$value" ;;
          --home=*) HOME_DIR="$value" ;;
          --bin-dir=*) BIN_DIR="$value" ;;
        esac
        shift
        ;;
      --uninstall)
        UNINSTALL=1
        shift
        ;;
      --help | -h)
        usage
        exit 0
        ;;
      *)
        die "unknown option: $1 (see --help)"
        ;;
    esac
  done

  if [ -z "$HOME_DIR" ]; then
    [ -n "${XDG_DATA_HOME:-}" ] || [ -n "${HOME:-}" ] || die "HOME is not set; pass --home"
    HOME_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/capstan"
  fi
  if [ -z "$BIN_DIR" ]; then
    [ -n "${HOME:-}" ] || die "HOME is not set; pass --bin-dir"
    BIN_DIR="$HOME/.local/bin"
  fi
  case "$HOME_DIR" in /*) ;; *) die "--home must be an absolute path: $HOME_DIR" ;; esac
  case "$BIN_DIR" in /*) ;; *) die "--bin-dir must be an absolute path: $BIN_DIR" ;; esac
  HOME_DIR="${HOME_DIR%/}"
  BIN_DIR="${BIN_DIR%/}"
  [ -n "$HOME_DIR" ] && [ -n "$BIN_DIR" ] || die "refusing to use / as an install directory"
  [ -z "$VERSION" ] || valid_version "$VERSION" || die "--version must be exactly x.y.z (digits only), got: $VERSION"
  case "$SHA256" in *[!0-9A-Fa-f]*) die "--sha256 must be hex" ;; esac
  BIN_LINK="$BIN_DIR/cstan"
  CURRENT="$HOME_DIR/current"
}

cleanup() {
  status=$?
  trap - EXIT
  if [ -n "$STAGING" ] && [ -d "$STAGING" ]; then
    rm -rf "$STAGING"
  fi
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
  exit "$status"
}

have() {
  command -v "$1" >/dev/null 2>&1
}

check_prereqs() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  case "$os" in
    Linux | Darwin) ;;
    *) die "unsupported OS: $os. Capstan supports Linux and macOS." ;;
  esac

  have node || die "node not found. Install Node.js $NODE_MAJOR_REQUIRED (for example with nvm or fnm) and re-run."
  node_version="$(node -v 2>/dev/null </dev/null || true)"
  node_major="${node_version#v}"
  node_major="${node_major%%.*}"
  case "$node_major" in
    "" | *[!0-9]*) die "could not read the Node version (node -v printed: $node_version). Capstan needs Node $NODE_MAJOR_REQUIRED." ;;
  esac
  [ "$node_major" = "$NODE_MAJOR_REQUIRED" ] ||
    die "Node $NODE_MAJOR_REQUIRED is required, found $node_version. Switch with: nvm install $NODE_MAJOR_REQUIRED && nvm use $NODE_MAJOR_REQUIRED (or fnm use $NODE_MAJOR_REQUIRED), then re-run."
  have npm || die "npm not found. It ships with Node.js $NODE_MAJOR_REQUIRED; reinstall Node."

  if [ "${CAPSTAN_FETCHER:-}" = wget ] && have wget; then
    FETCHER=wget # test hook: force the wget code path
  elif have curl; then
    FETCHER=curl
  elif have wget; then
    FETCHER=wget
  elif [ -z "$TARBALL" ] || is_url "$TARBALL"; then
    die "curl or wget is required to download Capstan."
  else
    FETCHER=""
  fi

  if [ "$os" = Darwin ]; then
    hint="Run: xcode-select --install"
  else
    hint="On Debian/Ubuntu: sudo apt install build-essential python3. On Fedora: sudo dnf install make gcc-c++ python3."
  fi
  have make || die "make not found; it is needed to compile the native fs-ext module. $hint"
  if ! have c++ && ! have g++ && ! have clang++; then
    die "no C++ compiler (c++, g++ or clang++) found; it is needed to compile the native fs-ext module. $hint"
  fi
  have python3 || die "python3 not found; node-gyp needs it to compile the native fs-ext module. $hint"

  have git || warn "git not found. Capstan needs git to run in a repository."
  have herdr || warn "herdr not found. Install Herdr; cstan start launches agents in its panes."
  have claude || warn "claude not found. Install Claude Code; the starter config uses it as the agent host."
}

is_url() {
  case "$1" in
    http://* | https://* | file://*) return 0 ;;
    *) return 1 ;;
  esac
}

# fetch <url> <dest>
fetch() {
  case "$FETCHER" in
    curl) curl -fsSL --retry 2 -o "$2" "$1" </dev/null || return 1 ;;
    wget)
      case "$1" in file://*) die "file:// URLs need curl; wget cannot fetch them." ;; esac
      wget -q -O "$2" "$1" </dev/null || return 1
      ;;
    *) return 1 ;;
  esac
}

resolve_latest_version() {
  latest_url="$RELEASE_BASE/latest"
  case "$FETCHER" in
    curl)
      final="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$latest_url" </dev/null)" ||
        die "could not look up the latest release at $latest_url. Pass --version <x.y.z>."
      ;;
    wget)
      final="$(wget --max-redirect=0 -S -O /dev/null "$latest_url" 2>&1 </dev/null |
        sed -n 's/^ *[Ll]ocation: *//p' | tr -d '\r' | tail -n 1 | awk '{ print $1 }')" || true
      ;;
  esac
  tag="${final##*/}"
  case "$tag" in
    v*) VERSION="${tag#v}" ;;
    *) VERSION="" ;;
  esac
  valid_version "$VERSION" ||
    die "could not read a release version from $latest_url (got: ${final:-nothing}). Pass --version <x.y.z>."
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    die "sha256sum or shasum is required to verify the download."
  fi
}

lower() {
  printf '%s' "$1" | tr 'A-F' 'a-f'
}

# Fetches the tarball into TMP_DIR and sets TGZ.
obtain_tarball() {
  if [ -n "$TARBALL" ]; then
    TGZ="$TMP_DIR/capstan-controller.tgz"
    if is_url "$TARBALL"; then
      say "Downloading $TARBALL"
      fetch "$TARBALL" "$TGZ" || die "download failed: $TARBALL"
    else
      [ -f "$TARBALL" ] || die "tarball not found: $TARBALL"
      cp "$TARBALL" "$TGZ"
    fi
    if [ -n "$SHA256" ]; then
      [ "$(sha256_of "$TGZ")" = "$(lower "$SHA256")" ] ||
        die "sha256 mismatch for $TARBALL; refusing to install."
      say "Checksum verified."
    else
      warn "installing a tarball without --sha256: it is unverified."
    fi
    return 0
  fi

  [ -n "$VERSION" ] || resolve_latest_version
  asset="capstan-controller-$VERSION.tgz"
  base="$RELEASE_BASE/download/v$VERSION"
  TGZ="$TMP_DIR/$asset"
  say "Downloading Capstan $VERSION from $base"
  fetch "$base/$asset" "$TGZ" || die "could not download $base/$asset. Check the version exists."
  fetch "$base/SHA256SUMS" "$TMP_DIR/SHA256SUMS" || die "could not download $base/SHA256SUMS; refusing to install without it."
  expected="$(awk -v a="$asset" '{ n = $2; sub(/^\*/, "", n); sub(/.*\//, "", n); if (n == a) { print $1; exit } }' "$TMP_DIR/SHA256SUMS")"
  [ -n "$expected" ] || die "SHA256SUMS has no entry for $asset; refusing to install."
  if [ -n "$SHA256" ] && [ "$(lower "$SHA256")" != "$(lower "$expected")" ]; then
    die "--sha256 does not match SHA256SUMS for $asset; refusing to install."
  fi
  [ "$(sha256_of "$TGZ")" = "$(lower "$expected")" ] ||
    die "sha256 mismatch for $asset; refusing to install."
  say "Checksum verified against SHA256SUMS."
}

installed_version() {
  [ -x "$CURRENT/bin/cstan" ] || return 0
  "$CURRENT/bin/cstan" --version 2>/dev/null </dev/null | sed -n 's/^cstan //p' | head -n 1 || true
}

install_files() {
  previous="$(installed_version)"
  mkdir -p "$HOME_DIR" "$BIN_DIR"
  STAGING="$HOME_DIR/staging.$$"
  rm -rf "$STAGING"

  if [ -L "$BIN_LINK" ]; then
    target="$(readlink "$BIN_LINK")"
    case "$target" in
      "$HOME_DIR"/*) ;;
      *) die "$BIN_LINK already exists and points to $target, outside $HOME_DIR. Remove it or pick another --bin-dir." ;;
    esac
  elif [ -e "$BIN_LINK" ]; then
    die "$BIN_LINK already exists and is not a symlink from a Capstan install. Remove it or pick another --bin-dir."
  fi

  say "Installing (this compiles the native fs-ext module, which can take a minute)..."
  npm_config_update_notifier=false npm install --global --prefix "$STAGING" \
    --omit=dev --no-audit --no-fund "$TGZ" </dev/null ||
    die "npm install failed; the previous install (if any) is unchanged. If the error is a compile error, check the C/C++ toolchain and python3."

  staged_out="$("$STAGING/bin/cstan" --version </dev/null 2>&1)" ||
    die "the staged cstan does not run: $staged_out"
  staged_version="$(printf '%s\n' "$staged_out" | sed -n 's/^cstan //p' | head -n 1)"
  [ -n "$staged_version" ] || die "the staged cstan --version printed '$staged_out', expected 'cstan <version>'."
  if [ -n "$VERSION" ] && [ "$staged_version" != "$VERSION" ]; then
    die "the tarball contains cstan $staged_version, expected $VERSION."
  fi
  VERSION="$staged_version"

  if [ -n "$previous" ]; then
    if [ "$previous" = "$VERSION" ]; then
      say "Capstan $VERSION is already installed; reinstalling to refresh the build."
    else
      say "Upgrading Capstan $previous -> $VERSION."
    fi
  fi

  rm -rf "$HOME_DIR/current.old"
  if [ -e "$CURRENT" ] || [ -L "$CURRENT" ]; then
    mv "$CURRENT" "$HOME_DIR/current.old"
  fi
  if ! mv "$STAGING" "$CURRENT"; then
    if [ -e "$HOME_DIR/current.old" ]; then
      mv "$HOME_DIR/current.old" "$CURRENT" || true
    fi
    die "could not move the new build into $CURRENT."
  fi
  STAGING=""
  rm -rf "$HOME_DIR/current.old"

  ln -sfn "$CURRENT/bin/cstan" "$BIN_LINK"

  final_out="$("$BIN_LINK" --version </dev/null 2>&1)" || die "installed cstan does not run: $final_out"
  [ "$final_out" = "cstan $VERSION" ] || die "installed cstan printed '$final_out', expected 'cstan $VERSION'."
}

report_success() {
  say ""
  say "Installed cstan $VERSION"
  say "  command: $BIN_LINK -> $CURRENT/bin/cstan"
  case ":${PATH:-}:" in
    *":$BIN_DIR:"*) ;;
    *)
      warn "$BIN_DIR is not on your PATH. Add it:"
      say "    export PATH=\"$BIN_DIR:\$PATH\""
      ;;
  esac
  found="$(command -v cstan 2>/dev/null || true)"
  if [ -n "$found" ] && [ "$found" != "$BIN_LINK" ]; then
    warn "another cstan comes first on your PATH: $found (it shadows $BIN_LINK)."
  fi
  say ""
  say "Next steps:"
  say "  cd into a git repository, then:"
  say "    cstan init"
  say "    cstan config check"
  say "    cstan start"
  say ""
  say "Notes:"
  say "  - Running controllers keep the old build until you run: cstan stop && cstan start"
  say "  - If you change Node major version, re-run this installer: fs-ext is compiled for that Node."
}

do_uninstall() {
  say "Uninstalling Capstan from $HOME_DIR"
  if [ -L "$BIN_LINK" ]; then
    target="$(readlink "$BIN_LINK")"
    case "$target" in
      "$HOME_DIR"/*)
        rm -f "$BIN_LINK"
        say "Removed $BIN_LINK"
        ;;
      *) warn "$BIN_LINK points to $target, outside $HOME_DIR; left alone." ;;
    esac
  elif [ -e "$BIN_LINK" ]; then
    warn "$BIN_LINK is not a symlink from a Capstan install; left alone."
  fi
  rm -rf "$CURRENT" "$HOME_DIR/current.old"
  for dir in "$HOME_DIR"/staging.*; do
    [ -e "$dir" ] && rm -rf "$dir"
  done
  rmdir "$HOME_DIR" 2>/dev/null || true
  say "Removed $CURRENT and any staging directories."
  say "Your projects' .capstan/ directories were not touched; delete them yourself if you want them gone."
}

main() {
  parse_args "$@"
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  if [ "$UNINSTALL" = 1 ]; then
    do_uninstall
    return 0
  fi
  check_prereqs
  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/capstan-install.XXXXXX")" || die "could not create a temp directory."
  obtain_tarball
  install_files
  report_success
}

main "$@"
