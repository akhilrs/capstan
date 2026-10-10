#!/bin/sh
# Capstan installer.
#
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/akhilrs/capstan/main/install.sh | sh -s -- --version 0.4.0
#
# Installs the two static Rust binaries of a release, cstan and cstan-dash, for Linux x64 or arm64. No Node.js, npm or
# compiler is needed. Releases before FIRST_RUST_VERSION shipped other things (a Node binary, or an npm package) and
# are installed by the install.sh of their own tag.
#
# POSIX sh. Everything lives in functions and `main "$@"` is the last line, so a
# truncated download defines functions and runs nothing. Never reads stdin,
# never uses sudo. Run with --help for the options.
set -eu

REPO_URL="https://github.com/akhilrs/capstan"
RAW_URL="https://raw.githubusercontent.com/akhilrs/capstan"
# The first release cut after the Rust cutover. Anything older is installed by that tag's own install.sh.
FIRST_RUST_VERSION=0.4.0

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
  cat <<'EOF2'
Install the Capstan controller (the `cstan` command and the `cstan-dash` dashboard).

Usage: install.sh [options]

Options:
  --version <x.y.z>     Release to install (env CAPSTAN_VERSION). Default: the latest release.
                        Releases before 0.4.0 are installed by the install.sh of their own tag.
  --binary <path|url>   Install this cstan binary instead of a release (env CAPSTAN_BINARY).
                        Verified against --sha256 or the SHA256SUMS file next to it.
  --dash-binary <file>  Install this cstan-dash next to cstan. Verified against the SHA256SUMS file next to it
                        when there is one.
  --no-dash             Do not install cstan-dash.
  --sha256 <hex>        Expected sha256 of the cstan binary (env CAPSTAN_SHA256).
  --home <dir>          Install root (env CAPSTAN_HOME).
                        Default: ${XDG_DATA_HOME:-$HOME/.local/share}/capstan
  --bin-dir <dir>       Where the cstan symlink goes (env CAPSTAN_BIN_DIR).
                        Default: $HOME/.local/bin
  --uninstall           Remove the install and the bin symlink.
  --help                Show this help.

A release lists cstan-<version>-linux-<x64|arm64> and cstan-dash-<version>-linux-<x64|arm64> in SHA256SUMS; both are
downloaded and checked against it. The install is <home>/current/bin/{cstan,cstan-dash}.
Re-running the installer upgrades, and replaces an install from an earlier layout (the Node binary, the front end with
cstan-node, or the npm global prefix). Project .capstan/ directories are never touched.
Environment: CAPSTAN_RELEASE_BASE overrides https://github.com/akhilrs/capstan/releases (tests).
EOF2
}

# True only for digits.digits.digits with nothing before or after.
valid_version() {
  case "$1" in
    "" | *[!0-9.]* | .* | *. | *..*) return 1 ;;
  esac
  [ "$(printf '%s' "$1" | tr -cd . | wc -c | tr -d ' ')" = 2 ]
}

# version_lt <a> <b>: true when a < b (both valid_version).
version_lt() {
  a1=${1%%.*}
  rest=${1#*.}
  a2=${rest%%.*}
  a3=${rest#*.}
  b1=${2%%.*}
  rest=${2#*.}
  b2=${rest%%.*}
  b3=${rest#*.}
  [ "$a1" -eq "$b1" ] || { [ "$a1" -lt "$b1" ]; return; }
  [ "$a2" -eq "$b2" ] || { [ "$a2" -lt "$b2" ]; return; }
  [ "$a3" -lt "$b3" ]
}

parse_args() {
  VERSION="${CAPSTAN_VERSION:-}"
  BINARY="${CAPSTAN_BINARY:-}"
  DASH_BINARY=""
  NO_DASH=0
  SHA256="${CAPSTAN_SHA256:-}"
  HOME_DIR="${CAPSTAN_HOME:-}"
  BIN_DIR="${CAPSTAN_BIN_DIR:-}"
  RELEASE_BASE="${CAPSTAN_RELEASE_BASE:-$REPO_URL/releases}"
  UNINSTALL=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --version | --binary | --dash-binary | --sha256 | --home | --bin-dir)
        [ $# -ge 2 ] || die "$1 needs a value"
        case "$1" in
          --version) VERSION="$2" ;;
          --binary) BINARY="$2" ;;
          --dash-binary) DASH_BINARY="$2" ;;
          --sha256) SHA256="$2" ;;
          --home) HOME_DIR="$2" ;;
          --bin-dir) BIN_DIR="$2" ;;
        esac
        shift 2
        ;;
      --version=* | --binary=* | --dash-binary=* | --sha256=* | --home=* | --bin-dir=*)
        value="${1#*=}"
        case "$1" in
          --version=*) VERSION="$value" ;;
          --binary=*) BINARY="$value" ;;
          --dash-binary=*) DASH_BINARY="$value" ;;
          --sha256=*) SHA256="$value" ;;
          --home=*) HOME_DIR="$value" ;;
          --bin-dir=*) BIN_DIR="$value" ;;
        esac
        shift
        ;;
      --no-dash)
        NO_DASH=1
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
  [ -z "$DASH_BINARY" ] || [ "$NO_DASH" = 0 ] || die "--dash-binary and --no-dash are exclusive"
  case "$HOME_DIR" in /*) ;; *) die "--home must be an absolute path: $HOME_DIR" ;; esac
  case "$BIN_DIR" in /*) ;; *) die "--bin-dir must be an absolute path: $BIN_DIR" ;; esac
  HOME_DIR="${HOME_DIR%/}"
  BIN_DIR="${BIN_DIR%/}"
  if [ -z "$HOME_DIR" ] || [ -z "$BIN_DIR" ]; then
    die "refusing to use / as an install directory"
  fi
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

is_url() {
  case "$1" in
    http://* | https://* | file://*) return 0 ;;
    *) return 1 ;;
  esac
}

check_prereqs() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  case "$os" in
    Linux) ;;
    *) die "unsupported OS: $os. Capstan publishes Linux binaries (x64 and arm64)." ;;
  esac

  if [ "${CAPSTAN_FETCHER:-}" = wget ] && have wget; then
    FETCHER=wget # test hook: force the wget code path
  elif have curl; then
    FETCHER=curl
  elif have wget; then
    FETCHER=wget
  else
    FETCHER=""
  fi
  # Only local --binary and --dash-binary files install without a download.
  if [ -z "$FETCHER" ] && { [ -z "$BINARY" ] || is_url "$BINARY"; }; then
    die "curl or wget is required to download Capstan."
  fi

  have git || warn "git not found. Capstan needs git to run in a repository."
  have herdr || warn "herdr not found. Install Herdr; cstan start launches agents in its panes."
  have claude || warn "claude not found. Install Claude Code; the starter config uses it as the agent host."
}

# Sets PLATFORM to linux-<x64|arm64> as the release names its binaries; dies when none is published for this machine.
detect_platform() {
  case "$(uname -m 2>/dev/null)" in
    x86_64 | amd64) plat_arch=x64 ;;
    aarch64 | arm64) plat_arch=arm64 ;;
    *) die "no Capstan binary for the CPU '$(uname -m 2>/dev/null)'. Releases have linux-x64 and linux-arm64." ;;
  esac
  PLATFORM="linux-$plat_arch"
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

old_installer_hint() {
  say "  curl -fsSL $RAW_URL/v$1/install.sh | sh -s -- --version $1"
}

# Settles VERSION for a release install and refuses one this installer cannot install.
choose_version() {
  if [ -n "$VERSION" ]; then
    if version_lt "$VERSION" "$FIRST_RUST_VERSION"; then
      {
        printf 'error: Capstan %s predates the Rust release %s, which is the first this installer installs.\n' "$VERSION" "$FIRST_RUST_VERSION"
        printf 'Run that tag'"'"'s own installer instead:\n'
        old_installer_hint "$VERSION"
      } >&2
      exit 1
    fi
    return 0
  fi
  resolve_latest_version
  if version_lt "$VERSION" "$FIRST_RUST_VERSION"; then
    {
      printf 'error: the latest published Capstan release is %s. The first release this installer installs, %s, is not out yet.\n' "$VERSION" "$FIRST_RUST_VERSION"
      printf 'Install %s with the installer of its own tag:\n' "$VERSION"
      old_installer_hint "$VERSION"
    } >&2
    exit 1
  fi
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

# Fetches cstan into TMP_DIR, verifies it and sets BIN_FILE. A release is checked against its SHA256SUMS (which also
# lists cstan-dash); a local --binary against --sha256 or the SHA256SUMS next to it.
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
    say "Downloading Capstan $VERSION ($PLATFORM) from $base"
    fetch "$base/SHA256SUMS" "$TMP_DIR/SHA256SUMS" || die "could not download $base/SHA256SUMS; refusing to install without it. Check the version exists."
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

# Fetches cstan-dash into TMP_DIR, verifies it and sets DASH_FILE (empty when none is installed). A release without a
# cstan-dash for this machine is incomplete and refused.
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
  [ -z "$BINARY" ] || return 0 # a local cstan binary installs alone unless --dash-binary names a dashboard
  dash_base="$RELEASE_BASE/download/v$VERSION"
  dash_name="cstan-dash-$VERSION-$PLATFORM"
  dash_expected="$(sums_entry "$TMP_DIR/SHA256SUMS" "$dash_name")"
  [ -n "$dash_expected" ] || die "SHA256SUMS has no entry for $dash_name; refusing to install an incomplete release."
  DASH_FILE="$TMP_DIR/cstan-dash"
  fetch "$dash_base/$dash_name" "$DASH_FILE" || die "could not download $dash_base/$dash_name."
  [ "$(sha256_of "$DASH_FILE")" = "$(lower "$dash_expected")" ] ||
    die "sha256 mismatch for $dash_name; refusing to install."
  say "cstan-dash checksum verified against SHA256SUMS."
}

installed_version() {
  [ -x "$CURRENT/bin/cstan" ] || return 0
  "$CURRENT/bin/cstan" --version 2>/dev/null </dev/null | sed -n 's/^cstan //p' | head -n 1 || true
}

# Refuses to replace a bin-dir entry that is not ours, and notes the earlier install.
prepare_install() {
  previous="$(installed_version)"
  OLD_LAYOUT=""
  if [ -e "$CURRENT/bin/cstan-node" ]; then
    OLD_LAYOUT="the front end with cstan-node"
  elif [ -d "$CURRENT/lib/node_modules" ]; then
    OLD_LAYOUT="the npm global prefix"
  elif [ -x "$CURRENT/bin/cstan" ]; then
    OLD_LAYOUT="the Node standalone binary or an earlier Rust install"
  fi
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

# The install is <staging>/bin/cstan and <staging>/bin/cstan-dash; nothing else goes in, so swapping it in replaces
# whatever layout was there before (cstan-node, lib/node_modules, ...).
stage_binaries() {
  say "Installing the Rust binaries..."
  mkdir -p "$STAGING/bin"
  cp "$BIN_FILE" "$STAGING/bin/cstan"
  chmod 755 "$STAGING/bin/cstan"
  [ -n "$DASH_FILE" ] || return 0
  cp "$DASH_FILE" "$STAGING/bin/cstan-dash"
  chmod 755 "$STAGING/bin/cstan-dash"
  dash_out="$("$STAGING/bin/cstan-dash" --version </dev/null 2>&1)" ||
    die "the staged cstan-dash does not run on this machine: $dash_out"
  case "$dash_out" in
    "cstan-dash "*) ;;
    *) die "the staged cstan-dash --version printed '$dash_out', expected 'cstan-dash <version>'." ;;
  esac
  dash_version="${dash_out#cstan-dash }"
  if [ -n "$VERSION" ] && [ "$dash_version" != "$VERSION" ]; then
    die "the download contains cstan-dash $dash_version, expected $VERSION."
  fi
}

# Warns about processes still running from the install that is about to be replaced (a project's controller daemon
# started by an earlier cstan keeps running its old files after the swap).
warn_running_daemons() {
  [ -d /proc/self ] || return 0
  running=0
  for proc in /proc/[0-9]*; do
    [ -L "$proc/exe" ] || continue
    exe="$(readlink "$proc/exe" 2>/dev/null || true)"
    exe="${exe% (deleted)}"
    case "$exe" in
      "$CURRENT"/bin/* | "$CURRENT"/lib/*) running=$((running + 1)) ;;
      *)
        # The npm layout ran node on the entry point.
        if tr '\0' ' ' <"$proc/cmdline" 2>/dev/null | grep -q -F "$CURRENT/lib/node_modules/"; then running=$((running + 1)); fi
        ;;
    esac
  done
  [ "$running" -gt 0 ] || return 0
  warn "$running process(es) of the previous install in $CURRENT are still running, most likely a project's controller daemon."
  say "  They keep running the old build. In each project, run: cstan stop && cstan start"
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
  if [ -n "$OLD_LAYOUT" ]; then
    say "Replacing the earlier install layout ($OLD_LAYOUT) with the Rust binaries."
  fi
  warn_running_daemons

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
  if [ -z "$BINARY" ]; then choose_version; fi
  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/capstan-install.XXXXXX")" || die "could not create a temp directory."
  obtain_binary
  obtain_dash
  prepare_install
  stage_binaries
  activate_staged
  report_success
}

main "$@"
