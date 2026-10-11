#!/bin/sh
# Builds cstan (rust/ workspace, package cstan-front) and cstan-dash (dash/) from this checkout and installs them,
# with the man page, through install.sh: one command for a local build.
#
# Usage: scripts/install-local.sh [--debug] [--home <dir>] [--bin-dir <dir>] [--man-dir <dir>] [--no-dash]
#        scripts/install-local.sh --uninstall [--home <dir>] [--bin-dir <dir>] [--man-dir <dir>]
#   --debug      Build the debug profile instead of release.
#   --no-dash    Build and install cstan only.
#   --uninstall  Run install.sh --uninstall (no build).
#   --home, --bin-dir, --man-dir  Passed to install.sh (their env vars CAPSTAN_HOME, CAPSTAN_BIN_DIR, CAPSTAN_MAN_DIR work too).
# Environment (tests): CAPSTAN_LOCAL_SKIP_BUILD=1 skips cargo and installs CAPSTAN_LOCAL_CSTAN and
# CAPSTAN_LOCAL_DASH (paths to prebuilt binaries) instead.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    die "sha256sum or shasum is required."
  fi
}

# Read-only: a running `cstan daemon` keeps the binary it started from until it is restarted. Signals nothing.
warn_running_controllers() {
  [ -d /proc/self ] || return 0
  n=0
  for proc in /proc/[0-9]*; do
    args="$(tr '\0' ' ' <"$proc/cmdline" 2>/dev/null || true)"
    case "$args" in
      *cstan" daemon"* | *cstan-daemon*) n=$((n + 1)) ;;
    esac
  done
  [ "$n" -gt 0 ] || return 0
  warn "$n Capstan controller process(es) look like they are running. They keep the old binary until restarted."
  say "  In each project, run: cstan stop && cstan start"
}

UNINSTALL=0
NO_DASH=0
DEBUG=0
HOME_DIR="${CAPSTAN_HOME:-}"
BIN_DIR="${CAPSTAN_BIN_DIR:-}"
# The options for install.sh are rotated to the end of "$@" as they are read (so paths with spaces survive);
# `todo` counts the arguments not read yet.
todo=$#
while [ "$todo" -gt 0 ]; do
  case "$1" in
    --debug) DEBUG=1 ;;
    --no-dash) NO_DASH=1 ;;
    --uninstall) UNINSTALL=1 ;;
    --home | --bin-dir | --man-dir)
      [ "$todo" -ge 2 ] || die "$1 needs a value"
      [ "$1" != --home ] || HOME_DIR="$2"
      [ "$1" != --bin-dir ] || BIN_DIR="$2"
      set -- "$@" "$1" "$2"
      shift 2
      todo=$((todo - 2))
      continue
      ;;
    --home=* | --bin-dir=* | --man-dir=*)
      case "$1" in
        --home=*) HOME_DIR="${1#*=}" ;;
        --bin-dir=*) BIN_DIR="${1#*=}" ;;
      esac
      set -- "$@" "$1"
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
  todo=$((todo - 1))
done
# "$@" is now exactly the pass-through options.

if [ "$UNINSTALL" = 1 ]; then
  exec sh "$ROOT/install.sh" --uninstall "$@"
fi

if [ "${CAPSTAN_LOCAL_SKIP_BUILD:-0}" = 1 ]; then
  CSTAN="${CAPSTAN_LOCAL_CSTAN:-}"
  DASH="${CAPSTAN_LOCAL_DASH:-}"
  [ -f "$CSTAN" ] || die "CAPSTAN_LOCAL_SKIP_BUILD=1 needs CAPSTAN_LOCAL_CSTAN to be a file."
  [ "$NO_DASH" = 1 ] || [ -f "$DASH" ] || die "CAPSTAN_LOCAL_SKIP_BUILD=1 needs CAPSTAN_LOCAL_DASH to be a file (or --no-dash)."
else
  command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH; install Rust with rustup (https://rustup.rs)."
  if [ "$DEBUG" = 1 ]; then
    profile=debug
    flag=""
  else
    profile=release
    flag="--release"
  fi
  say "Building cstan ($profile) in rust/ ..."
  # shellcheck disable=SC2086
  cargo build $flag --locked --manifest-path "$ROOT/rust/Cargo.toml" -p cstan-front --bin cstan
  CSTAN="$ROOT/rust/target/$profile/cstan"
  DASH=""
  if [ "$NO_DASH" = 0 ]; then
    say "Building cstan-dash ($profile) in dash/ ..."
    # shellcheck disable=SC2086
    cargo build $flag --locked --manifest-path "$ROOT/dash/Cargo.toml"
    DASH="$ROOT/dash/target/$profile/cstan-dash"
  fi
  [ -x "$CSTAN" ] || die "the build did not produce $CSTAN."
  [ "$NO_DASH" = 1 ] || [ -x "$DASH" ] || die "the build did not produce $DASH."
fi

warn_running_controllers

SHA="$(sha256_of "$CSTAN")"
if [ "$NO_DASH" = 1 ]; then
  sh "$ROOT/install.sh" --binary "$CSTAN" --sha256 "$SHA" --no-dash "$@"
else
  sh "$ROOT/install.sh" --binary "$CSTAN" --sha256 "$SHA" --dash-binary "$DASH" "$@"
fi

# install.sh printed the paths, the man page and the PATH hint above; add the version of what is now installed.
if [ -z "$HOME_DIR" ]; then
  HOME_DIR="${XDG_DATA_HOME:-${HOME:-}/.local/share}/capstan"
fi
installed="$("$HOME_DIR/current/bin/cstan" --version </dev/null 2>&1 || true)"
say ""
say "Local build installed: $installed (built from $ROOT)"
