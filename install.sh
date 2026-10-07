#!/bin/sh
# Capstan installer.
#
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --version 0.1.1
#
# Installs the standalone binary (no Node.js needed) when the release has one for this machine, otherwise
# the npm tarball (needs Node 24 and npm).
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
  --binary <path|url>   Install this standalone binary instead of a release (env CAPSTAN_BINARY).
                        Verified against --sha256 or the SHA256SUMS file next to it.
  --no-binary           Install the npm tarball even when the release has a binary for this machine.
  --dash-binary <file>  Install this cstan-dash (the Rust dashboard) next to cstan. Verified against the
                        SHA256SUMS file next to it when there is one.
  --no-dash             Do not install cstan-dash even when the release has one.
  --front-binary <file> Install this cstan front end (the Rust cstan-front) as bin/cstan, with the standalone
                        binary as bin/cstan-node. Verified against the SHA256SUMS file next to it when there is one.
  --no-front            Do not install the front end even when the release has one; the binary is bin/cstan.
  --tarball <path|url>  Install this npm tarball instead of a release (env CAPSTAN_TARBALL).
  --sha256 <hex>        Expected sha256 of the binary or tarball (env CAPSTAN_SHA256).
  --home <dir>          Install root (env CAPSTAN_HOME).
                        Default: ${XDG_DATA_HOME:-$HOME/.local/share}/capstan
  --bin-dir <dir>       Where the cstan symlink goes (env CAPSTAN_BIN_DIR).
                        Default: $HOME/.local/bin
  --uninstall           Remove the install and the bin symlink.
  --help                Show this help.

A release that lists cstan-dash-<version>-<os>-<arch> in SHA256SUMS also gets cstan-dash, checked the same way;
without it `cstan dash` runs the Node dashboard.
A release that lists cstan-front-<version>-<os>-<arch> also gets the native front end: the binary install then holds
bin/cstan (the front end, which serves agent commands itself and hands every other command to bin/cstan-node, the
standalone binary). A front end that does not run here is dropped and the binary is installed as bin/cstan, as before.
The binary needs no Node.js. The tarball path needs Node 24 and npm.
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
  BINARY="${CAPSTAN_BINARY:-}"
  NO_BINARY=0
  DASH_BINARY=""
  NO_DASH=0
  FRONT_BINARY=""
  NO_FRONT=0
  SHA256="${CAPSTAN_SHA256:-}"
  HOME_DIR="${CAPSTAN_HOME:-}"
  BIN_DIR="${CAPSTAN_BIN_DIR:-}"
  RELEASE_BASE="${CAPSTAN_RELEASE_BASE:-$REPO_URL/releases}"
  UNINSTALL=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --version | --tarball | --binary | --dash-binary | --front-binary | --sha256 | --home | --bin-dir)
        [ $# -ge 2 ] || die "$1 needs a value"
        case "$1" in
          --version) VERSION="$2" ;;
          --tarball) TARBALL="$2" ;;
          --binary) BINARY="$2" ;;
          --dash-binary) DASH_BINARY="$2" ;;
          --front-binary) FRONT_BINARY="$2" ;;
          --sha256) SHA256="$2" ;;
          --home) HOME_DIR="$2" ;;
          --bin-dir) BIN_DIR="$2" ;;
        esac
        shift 2
        ;;
      --version=* | --tarball=* | --binary=* | --dash-binary=* | --front-binary=* | --sha256=* | --home=* | --bin-dir=*)
        value="${1#*=}"
        case "$1" in
          --version=*) VERSION="$value" ;;
          --tarball=*) TARBALL="$value" ;;
          --binary=*) BINARY="$value" ;;
          --dash-binary=*) DASH_BINARY="$value" ;;
          --front-binary=*) FRONT_BINARY="$value" ;;
          --sha256=*) SHA256="$value" ;;
          --home=*) HOME_DIR="$value" ;;
          --bin-dir=*) BIN_DIR="$value" ;;
        esac
        shift
        ;;
      --no-binary)
        NO_BINARY=1
        shift
        ;;
      --no-dash)
        NO_DASH=1
        shift
        ;;
      --no-front)
        NO_FRONT=1
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
  [ -z "$BINARY" ] || [ -z "$TARBALL" ] || die "--binary and --tarball are exclusive"
  [ -z "$BINARY" ] || [ "$NO_BINARY" = 0 ] || die "--binary and --no-binary are exclusive"
  [ -z "$DASH_BINARY" ] || [ "$NO_DASH" = 0 ] || die "--dash-binary and --no-dash are exclusive"
  [ -z "$FRONT_BINARY" ] || [ "$NO_FRONT" = 0 ] || die "--front-binary and --no-front are exclusive"
  [ -z "$FRONT_BINARY" ] || [ -z "$TARBALL" ] || die "--front-binary goes with the standalone binary, not --tarball"
  [ -z "$FRONT_BINARY" ] || [ "$NO_BINARY" = 0 ] || die "--front-binary goes with the standalone binary, not --no-binary"
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

  if [ "${CAPSTAN_FETCHER:-}" = wget ] && have wget; then
    FETCHER=wget # test hook: force the wget code path
  elif have curl; then
    FETCHER=curl
  elif have wget; then
    FETCHER=wget
  elif { [ -z "$TARBALL" ] || is_url "$TARBALL"; } && { [ -z "$BINARY" ] || is_url "$BINARY"; }; then
    die "curl or wget is required to download Capstan."
  else
    FETCHER=""
  fi

  have git || warn "git not found. Capstan needs git to run in a repository."
  have herdr || warn "herdr not found. Install Herdr; cstan start launches agents in its panes."
  have claude || warn "claude not found. Install Claude Code; the starter config uses it as the agent host."
}

# The npm tarball path is the only one that runs Node.
check_node() {
  have node || die "node not found. Install Node.js $NODE_MAJOR_REQUIRED (for example with nvm or fnm) and re-run, or install the standalone binary (it needs no Node)."
  node_version="$(node -v 2>/dev/null </dev/null || true)"
  node_major="${node_version#v}"
  node_major="${node_major%%.*}"
  case "$node_major" in
    "" | *[!0-9]*) die "could not read the Node version (node -v printed: $node_version). Capstan needs Node $NODE_MAJOR_REQUIRED." ;;
  esac
  [ "$node_major" = "$NODE_MAJOR_REQUIRED" ] ||
    die "Node $NODE_MAJOR_REQUIRED is required, found $node_version. Switch with: nvm install $NODE_MAJOR_REQUIRED && nvm use $NODE_MAJOR_REQUIRED (or fnm use $NODE_MAJOR_REQUIRED), then re-run."
  have npm || die "npm not found. It ships with Node.js $NODE_MAJOR_REQUIRED; reinstall Node."
}

# Sets PLATFORM to <os>-<arch> as the release names its binaries, or "" when none is published for this machine.
detect_platform() {
  case "$(uname -s 2>/dev/null)" in
    Linux) plat_os=linux ;;
    Darwin) plat_os=darwin ;;
    *) plat_os="" ;;
  esac
  case "$(uname -m 2>/dev/null)" in
    x86_64 | amd64) plat_arch=x64 ;;
    aarch64 | arm64) plat_arch=arm64 ;;
    *) plat_arch="" ;;
  esac
  PLATFORM=""
  [ -z "$plat_os" ] || [ -z "$plat_arch" ] || PLATFORM="$plat_os-$plat_arch"
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

# sums_entry <SHA256SUMS file> <name>: the hash listed for <name>, or nothing.
sums_entry() {
  awk -v a="$2" '{ n = $2; sub(/^\*/, "", n); sub(/.*\//, "", n); if (n == a) { print $1; exit } }' "$1"
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
  [ -f "$TMP_DIR/SHA256SUMS" ] || fetch "$base/SHA256SUMS" "$TMP_DIR/SHA256SUMS" || die "could not download $base/SHA256SUMS; refusing to install without it."
  expected="$(sums_entry "$TMP_DIR/SHA256SUMS" "$asset")"
  [ -n "$expected" ] || die "SHA256SUMS has no entry for $asset; refusing to install."
  if [ -n "$SHA256" ] && [ "$(lower "$SHA256")" != "$(lower "$expected")" ]; then
    die "--sha256 does not match SHA256SUMS for $asset; refusing to install."
  fi
  [ "$(sha256_of "$TGZ")" = "$(lower "$expected")" ] ||
    die "sha256 mismatch for $asset; refusing to install."
  say "Checksum verified against SHA256SUMS."
}

# Decides KIND (binary or tarball). A release is a binary install when its SHA256SUMS lists
# cstan-<version>-<os>-<arch> for this machine; releases without one keep the npm tarball path.
choose_kind() {
  if [ -n "$BINARY" ]; then
    KIND=binary
  elif [ -n "$TARBALL" ] || [ "$NO_BINARY" = 1 ] || [ -z "$PLATFORM" ]; then
    KIND=tarball
  else
    [ -n "$VERSION" ] || resolve_latest_version
    if fetch "$RELEASE_BASE/download/v$VERSION/SHA256SUMS" "$TMP_DIR/SHA256SUMS" &&
      [ -n "$(sums_entry "$TMP_DIR/SHA256SUMS" "cstan-$VERSION-$PLATFORM")" ]; then
      KIND=binary
    else
      rm -f "$TMP_DIR/SHA256SUMS"
      KIND=tarball
      say "No standalone binary for $PLATFORM in Capstan $VERSION; using the npm tarball."
    fi
  fi
}

# Fetches the binary into TMP_DIR, verifies it and sets BIN_FILE.
obtain_binary() {
  BIN_FILE="$TMP_DIR/cstan"
  if [ -n "$BINARY" ]; then
    name="${BINARY##*/}"
    sums_source="${BINARY%/*}/SHA256SUMS"
    if is_url "$BINARY"; then
      say "Downloading $BINARY"
      fetch "$BINARY" "$BIN_FILE" || die "download failed: $BINARY"
      fetch "$sums_source" "$TMP_DIR/SHA256SUMS" 2>/dev/null || rm -f "$TMP_DIR/SHA256SUMS"
    else
      [ -f "$BINARY" ] || die "binary not found: $BINARY"
      cp "$BINARY" "$BIN_FILE"
      if [ -f "$sums_source" ]; then cp "$sums_source" "$TMP_DIR/SHA256SUMS"; fi
    fi
  else
    name="cstan-$VERSION-$PLATFORM"
    base="$RELEASE_BASE/download/v$VERSION"
    say "Downloading Capstan $VERSION ($PLATFORM binary) from $base"
    fetch "$base/$name" "$BIN_FILE" || die "could not download $base/$name. Check the version exists."
  fi
  expected=""
  if [ -f "$TMP_DIR/SHA256SUMS" ]; then
    expected="$(sums_entry "$TMP_DIR/SHA256SUMS" "$name")"
    [ -n "$expected" ] || die "SHA256SUMS has no entry for $name; refusing to install."
  fi
  if [ -n "$SHA256" ] && [ -n "$expected" ] && [ "$(lower "$SHA256")" != "$(lower "$expected")" ]; then
    die "--sha256 does not match SHA256SUMS for $name; refusing to install."
  fi
  actual="$(sha256_of "$BIN_FILE")"
  if [ -n "$SHA256" ]; then
    [ "$actual" = "$(lower "$SHA256")" ] || die "sha256 mismatch for $name; refusing to install."
    say "Checksum verified."
  elif [ -n "$expected" ]; then
    [ "$actual" = "$(lower "$expected")" ] || die "sha256 mismatch for $name; refusing to install."
    say "Checksum verified against SHA256SUMS."
  else
    warn "installing a binary without --sha256 or a SHA256SUMS file next to it: it is unverified."
  fi
}

# Fetches cstan-dash into TMP_DIR, verifies it and sets DASH_FILE (empty when none is installed). Runs after the
# release is known, so VERSION and PLATFORM are set. A release without a cstan-dash asset installs cstan alone.
obtain_dash() {
  DASH_FILE=""
  [ "$NO_DASH" = 0 ] || return 0
  if [ -n "$DASH_BINARY" ]; then
    [ -f "$DASH_BINARY" ] || die "cstan-dash not found: $DASH_BINARY"
    DASH_FILE="$TMP_DIR/cstan-dash"
    cp "$DASH_BINARY" "$DASH_FILE"
    dash_name="${DASH_BINARY##*/}"
    dash_sums="${DASH_BINARY%/*}/SHA256SUMS"
    if [ -f "$dash_sums" ]; then
      dash_expected="$(sums_entry "$dash_sums" "$dash_name")"
      [ -n "$dash_expected" ] || die "SHA256SUMS has no entry for $dash_name; refusing to install."
      [ "$(sha256_of "$DASH_FILE")" = "$(lower "$dash_expected")" ] ||
        die "sha256 mismatch for $dash_name; refusing to install."
      say "cstan-dash checksum verified against SHA256SUMS."
    else
      warn "installing cstan-dash without a SHA256SUMS file next to it: it is unverified."
    fi
    return 0
  fi
  [ -z "$BINARY" ] && [ -z "$TARBALL" ] && [ -n "$PLATFORM" ] && [ -n "$VERSION" ] || return 0
  dash_base="$RELEASE_BASE/download/v$VERSION"
  [ -f "$TMP_DIR/SHA256SUMS" ] || fetch "$dash_base/SHA256SUMS" "$TMP_DIR/SHA256SUMS" || return 0
  dash_name="cstan-dash-$VERSION-$PLATFORM"
  dash_expected="$(sums_entry "$TMP_DIR/SHA256SUMS" "$dash_name")"
  if [ -z "$dash_expected" ]; then
    say "Note: Capstan $VERSION has no cstan-dash for $PLATFORM; installing cstan alone (cstan dash uses the Node dashboard)."
    return 0
  fi
  DASH_FILE="$TMP_DIR/cstan-dash"
  fetch "$dash_base/$dash_name" "$DASH_FILE" || die "could not download $dash_base/$dash_name."
  [ "$(sha256_of "$DASH_FILE")" = "$(lower "$dash_expected")" ] ||
    die "sha256 mismatch for $dash_name; refusing to install."
  say "cstan-dash checksum verified against SHA256SUMS."
}

# Fetches the front end into TMP_DIR, verifies it and sets FRONT_FILE (empty when none is installed). Only a binary
# install gets one: the front end hands every other command to the standalone binary beside it. A release without a
# cstan-front asset installs the binary alone.
obtain_front() {
  FRONT_FILE=""
  [ "$NO_FRONT" = 0 ] || return 0
  if [ -n "$FRONT_BINARY" ]; then
    [ -f "$FRONT_BINARY" ] || die "cstan front end not found: $FRONT_BINARY"
    FRONT_FILE="$TMP_DIR/cstan-front"
    cp "$FRONT_BINARY" "$FRONT_FILE"
    front_name="${FRONT_BINARY##*/}"
    front_sums="${FRONT_BINARY%/*}/SHA256SUMS"
    if [ -f "$front_sums" ]; then
      front_expected="$(sums_entry "$front_sums" "$front_name")"
      [ -n "$front_expected" ] || die "SHA256SUMS has no entry for $front_name; refusing to install."
      [ "$(sha256_of "$FRONT_FILE")" = "$(lower "$front_expected")" ] ||
        die "sha256 mismatch for $front_name; refusing to install."
      say "cstan front end checksum verified against SHA256SUMS."
    else
      warn "installing the cstan front end without a SHA256SUMS file next to it: it is unverified."
    fi
    return 0
  fi
  [ -z "$BINARY" ] && [ -n "$PLATFORM" ] && [ -n "$VERSION" ] || return 0
  front_base="$RELEASE_BASE/download/v$VERSION"
  [ -f "$TMP_DIR/SHA256SUMS" ] || fetch "$front_base/SHA256SUMS" "$TMP_DIR/SHA256SUMS" || return 0
  front_name="cstan-front-$VERSION-$PLATFORM"
  front_expected="$(sums_entry "$TMP_DIR/SHA256SUMS" "$front_name")"
  if [ -z "$front_expected" ]; then
    say "Note: Capstan $VERSION has no cstan front end for $PLATFORM; installing the binary alone."
    return 0
  fi
  FRONT_FILE="$TMP_DIR/cstan-front"
  fetch "$front_base/$front_name" "$FRONT_FILE" || die "could not download $front_base/$front_name."
  [ "$(sha256_of "$FRONT_FILE")" = "$(lower "$front_expected")" ] ||
    die "sha256 mismatch for $front_name; refusing to install."
  say "cstan front end checksum verified against SHA256SUMS."
}

installed_version() {
  [ -x "$CURRENT/bin/cstan" ] || return 0
  "$CURRENT/bin/cstan" --version 2>/dev/null </dev/null | sed -n 's/^cstan //p' | head -n 1 || true
}

# Refuses to replace a bin-dir entry that is not ours, and creates the staging directory.
prepare_install() {
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
}

stage_tarball() {
  say "Installing from the npm tarball..."
  npm_config_update_notifier=false npm install --global --prefix "$STAGING" \
    --omit=dev --no-audit --no-fund "$TGZ" </dev/null ||
    die "npm install failed; the previous install (if any) is unchanged."
}

# The binary is the whole install: <staging>/bin/cstan, the same layout the tarball install has.
stage_binary() {
  say "Installing the standalone binary..."
  mkdir -p "$STAGING/bin"
  cp "$BIN_FILE" "$STAGING/bin/cstan"
  chmod 755 "$STAGING/bin/cstan"
}

# With a verified front end the layout is bin/cstan (the front end) and bin/cstan-node (the standalone binary the front
# end hands everything else to). The front end is used only when `bin/cstan __front-version` runs here and names this
# release; otherwise it is dropped with a warning and the binary stays bin/cstan, the layout of earlier releases.
stage_front() {
  [ -n "$FRONT_FILE" ] || return 0
  cp "$FRONT_FILE" "$STAGING/bin/cstan-front.new"
  chmod 755 "$STAGING/bin/cstan-front.new"
  front_out="$("$STAGING/bin/cstan-front.new" __front-version </dev/null 2>/dev/null)" || front_out=""
  front_version="${front_out#cstan-front }"
  want_version="$VERSION"
  if [ -z "$want_version" ]; then
    want_version="$("$STAGING/bin/cstan" --version </dev/null 2>/dev/null | sed -n 's/^cstan //p' | head -n 1)" || want_version=""
  fi
  if [ "$front_version" = "$front_out" ] || [ -z "$front_version" ] || [ "$front_version" != "$want_version" ]; then
    rm -f "$STAGING/bin/cstan-front.new"
    FRONT_FILE=""
    warn "the cstan front end does not run on this machine or is not release $want_version; installed the binary alone."
    return 0
  fi
  mv "$STAGING/bin/cstan" "$STAGING/bin/cstan-node"
  mv "$STAGING/bin/cstan-front.new" "$STAGING/bin/cstan"
}

# Puts the verified cstan-dash beside cstan, where the resolver looks for it. A file that does not run (for example
# built for another CPU) is dropped with a warning rather than failing the whole install.
stage_dash() {
  [ -n "$DASH_FILE" ] || return 0
  mkdir -p "$STAGING/bin"
  cp "$DASH_FILE" "$STAGING/bin/cstan-dash"
  chmod 755 "$STAGING/bin/cstan-dash"
  if ! "$STAGING/bin/cstan-dash" --version </dev/null >/dev/null 2>&1; then
    rm -f "$STAGING/bin/cstan-dash"
    DASH_FILE=""
    warn "cstan-dash does not run on this machine; installed cstan alone."
  fi
}

# Runs the staged cstan, checks its version and swaps it in as current.
activate_staged() {
  staged_out="$("$STAGING/bin/cstan" --version </dev/null 2>&1)" ||
    die "the staged cstan does not run: $staged_out"
  staged_version="$(printf '%s\n' "$staged_out" | sed -n 's/^cstan //p' | head -n 1)"
  [ -n "$staged_version" ] || die "the staged cstan --version printed '$staged_out', expected 'cstan <version>'."
  if [ -n "$VERSION" ] && [ "$staged_version" != "$VERSION" ]; then
    die "the download contains cstan $staged_version, expected $VERSION."
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
  if [ -n "$FRONT_FILE" ]; then
    say "  front end: $CURRENT/bin/cstan (agent commands run natively; everything else runs $CURRENT/bin/cstan-node)"
  fi
  if [ -n "$DASH_FILE" ]; then
    say "  dashboard: $CURRENT/bin/cstan-dash (cstan dash uses it)"
  fi
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
  detect_platform
  # Without a binary in play Node is needed, so a wrong Node stops the install before any download.
  if [ -n "$TARBALL" ] || [ "$NO_BINARY" = 1 ]; then check_node; fi
  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/capstan-install.XXXXXX")" || die "could not create a temp directory."
  choose_kind
  if [ "$KIND" = binary ]; then
    obtain_binary
    obtain_front
    obtain_dash
    prepare_install
    stage_binary
    stage_front
    stage_dash
  else
    check_node
    obtain_tarball
    FRONT_FILE=""
    obtain_dash
    prepare_install
    stage_tarball
    stage_dash
  fi
  activate_staged
  report_success
}

main "$@"
