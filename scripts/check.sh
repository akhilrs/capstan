#!/bin/sh
# The single gate. Needs no node, npm or python on PATH: cargo, git and a POSIX shell.
#   - cargo fmt --check, clippy -D warnings and cargo test --locked for rust/ (the workspace), dash/ and tools/release; a test
#     run that leaves a daemon of its own build alive fails (scripts/daemon-survivors.sh)
#   - the release builds of cstan and cstan-dash, then scripts/smoke-binary.sh on them
#   - scripts/test-install.sh, scripts/check-release-workflow.sh, scripts/check-version.sh
#   - the live suites (a real Herdr, and a real Claude Code where a test needs one), only with CSTAN_LIVE=1
#   - the no-Node guard: git ls-files lists no TypeScript, .mjs/.cjs, package.json, lockfile, tsconfig, eslint or prettier file
#     outside scripts/node-allowlist; scripts/test-node-guard.sh proves the guard fails on an added one
# Usage: scripts/check.sh [stage]    no argument runs every stage, in this order; CI runs one job per stage
#   rust       fmt, clippy and test of the rust/ workspace
#   dash       fmt, clippy and test of dash/
#   tools      fmt, clippy and test of tools/release
#   scripts    smoke-binary.sh, test-install.sh, check-release-workflow.sh, check-version.sh, test-check-version.sh,
#              test-no-node-path.sh, the no-Node guard and test-node-guard.sh
#   release    the release builds of cstan and cstan-dash (also run by "all" when CSTAN_RELEASE_BUILD=1; the Release
#              workflow builds and verifies the real binaries on tags, so CI skips them)
#   node-guard only the no-Node guard (needs git, no cargo)
#   (test-map and --test-map were retired with the Node tree; they are refused as unknown stages)
# Environment: CSTAN_RELEASE_BUILD=1 (build the release binaries before the scripts stage so smoke-binary.sh runs on them),
#   CSTAN_NO_NODE=1 (before any stage, drop node, npm, npx, corepack and tsc from PATH and fail if one still resolves),
#   CARGO_TARGET_DIR (honoured by cargo; point it outside the checkout on a shared machine), CSTAN_CHECK_JOBS
#   (cargo -j; cargo's own default when unset), CSTAN_LIVE=1.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JOBS=""
if [ -n "${CSTAN_CHECK_JOBS:-}" ]; then
  case "$CSTAN_CHECK_JOBS" in
    *[!0-9]* | 0*) echo "check: CSTAN_CHECK_JOBS must be a positive number" >&2; exit 2 ;;
  esac
  JOBS="-j $CSTAN_CHECK_JOBS"
fi

step() { printf '\n== %s\n' "$1"; }

# cargo_gate <directory>: fmt, clippy and test of one cargo tree. With CARGO_TARGET_DIR set, each tree builds into its own
# subdirectory of it: one shared debug directory would put cstan-dash next to cstan, and the black-box suites (which
# expect no cstan-dash there) would then fail depending on which stage ran first.
cargo_gate() (
  dir="$1"
  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    CARGO_TARGET_DIR="${CARGO_TARGET_DIR%/}/gate-$(basename "$dir")"
    export CARGO_TARGET_DIR
  fi
  step "cargo fmt --check ($dir)"
  (cd "$ROOT/$dir" && cargo fmt --check)
  step "cargo clippy ($dir)"
  # shellcheck disable=SC2086
  (cd "$ROOT/$dir" && cargo clippy $JOBS --all-targets --locked -- -D warnings)
  step "cargo test ($dir)"
  # shellcheck disable=SC2086
  # The tests start daemons; the run fails if one of them is still alive when it ends (scripts/daemon-survivors.sh).
  "$ROOT/scripts/daemon-survivors.sh" check "${CARGO_TARGET_DIR:-$ROOT/$dir/target}" \
    sh -c 'cd "$1" && shift && exec cargo test "$@"' sh "$ROOT/$dir" $JOBS --locked
)

# ------------------------------------------------------------------------------------------------- the no-Node guard

# node_guard [repo]: no tracked file of repo (default: this checkout) is Node tooling or TypeScript, except under a prefix
# of scripts/node-allowlist (taken from the checkout that holds this script).
node_guard() {
  repo="${1:-$ROOT}"
  step "no-Node guard"
  hits="$(git -C "$repo" ls-files -- '*.ts' '*.tsx' '*.mjs' '*.cjs' '*package.json' '*package-lock.json' '*npm-shrinkwrap.json' \
    '*tsconfig*.json' '*eslint.config.*' '*.eslintrc*' '*prettier.config.*' '*.prettierrc*' |
    while IFS= read -r path; do
      keep=0
      while IFS= read -r prefix; do
        case "$prefix" in '' | '#'*) continue ;; esac
        case "$path" in "$prefix"*) keep=1; break ;; esac
      done <"$ROOT/scripts/node-allowlist"
      [ "$keep" = 1 ] || printf '%s\n' "$path"
    done)"
  if [ -n "$hits" ]; then
    echo "node-guard: tracked Node or TypeScript files outside scripts/node-allowlist:" >&2
    printf '  %s\n' $hits >&2
    return 1
  fi
  echo "node-guard: no tracked Node or TypeScript file outside scripts/node-allowlist"
}

STAGE="${1:-all}"
case "$STAGE" in
  all | rust | dash | tools | scripts | release | node-guard) ;;
  *) echo "usage: scripts/check.sh [rust|dash|tools|scripts|release|node-guard]" >&2; exit 2 ;;
esac
want() { [ "$STAGE" = all ] || [ "$STAGE" = "$1" ]; }

# ------------------------------------------------------------------------------------------- CSTAN_NO_NODE=1: no node on PATH

# A directory of PATH that holds node, nodejs, npm, npx, corepack or tsc is replaced by a scratch directory of symlinks to
# every other executable in it (so git, sh, cc and the coreutils of /usr/bin stay reachable); the other directories stay as
# they are. Then node, npm, npx and tsc must not resolve. The scratch mirror is removed on exit.
NODE_TOOLS="node nodejs npm npx corepack tsc"
NO_NODE_DIR=""
no_node_cleanup() { [ -z "$NO_NODE_DIR" ] || rm -rf "$NO_NODE_DIR"; }

no_node_path() {
  NO_NODE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/capstan-check-nonode.XXXXXX")"
  trap no_node_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  clean=""
  n=0
  old_ifs="$IFS"
  IFS=:
  for dir in $PATH; do
    IFS="$old_ifs"
    hit=0
    # An empty entry is the working directory: kept (as ".") and not mirrored, so a node there fails the check below.
    [ -n "$dir" ] || dir=.
    if [ "$dir" != . ]; then
      for tool in $NODE_TOOLS; do
        if [ -x "$dir/$tool" ] && [ ! -d "$dir/$tool" ]; then hit=1; break; fi
      done
    fi
    if [ "$hit" = 0 ]; then
      clean="${clean:+$clean:}$dir"
      continue
    fi
    n=$((n + 1))
    mirror="$NO_NODE_DIR/$n"
    mkdir "$mirror"
    count=0
    for f in "$dir"/*; do
      [ -f "$f" ] && [ -x "$f" ] || continue
      base="${f##*/}"
      case " $NODE_TOOLS " in *" $base "*) continue ;; esac
      ln -s "$f" "$mirror/$base"
      count=$((count + 1))
    done
    echo "no-node: $dir -> $mirror ($count executables linked)"
    clean="${clean:+$clean:}$mirror"
  done
  PATH="$clean"
  export PATH
  for tool in node npm npx tsc; do
    found="$(command -v "$tool" 2>/dev/null || true)"
    if [ -n "$found" ]; then
      echo "check: CSTAN_NO_NODE=1 but $tool still resolves ($found); it is not in a PATH directory the scrub could mirror" >&2
      exit 1
    fi
  done
  echo "no-node: PATH=$PATH"
}

if [ "${CSTAN_NO_NODE:-}" = "1" ]; then
  no_node_path
fi

if [ "$STAGE" = node-guard ]; then
  node_guard
  exit $?
fi

command -v cargo >/dev/null 2>&1 || {
  # shellcheck disable=SC2016
  echo 'check: cargo not found; export PATH="$HOME/.cargo/bin:$PATH"' >&2
  exit 1
}
# One toolchain for every cargo tree.
cmp -s "$ROOT/rust/rust-toolchain.toml" "$ROOT/dash/rust-toolchain.toml" || {
  echo "check: rust/rust-toolchain.toml differs from dash/rust-toolchain.toml; they must be identical" >&2
  exit 1
}

release_build() {
  step "release builds (cstan, cstan-dash)"
  # shellcheck disable=SC2086
  (cd "$ROOT/rust" && cargo build $JOBS --release --locked -p cstan-front)
  # shellcheck disable=SC2086
  (cd "$ROOT/dash" && cargo build $JOBS --release --locked)
}

! want rust || cargo_gate rust
! want dash || cargo_gate dash
! want tools || cargo_gate tools/release

if [ "$STAGE" = release ] || { [ "$STAGE" = all ] && [ "${CSTAN_RELEASE_BUILD:-}" = "1" ]; }; then
  release_build
  # smoke-binary.sh must then find the binaries just built: a missing one is a failure, not a skip.
  export CSTAN_REQUIRE_BINARIES=1
elif [ "$STAGE" = all ]; then
  echo
  echo "release builds: not run (set CSTAN_RELEASE_BUILD=1 or run scripts/check.sh release; the Release workflow builds them on tags)"
fi

if want scripts; then
  step "smoke-binary.sh"
  sh "$ROOT/scripts/smoke-binary.sh"
  step "test-install.sh"
  sh "$ROOT/scripts/test-install.sh"
  step "check-release-workflow.sh"
  sh "$ROOT/scripts/check-release-workflow.sh"
  step "check-version.sh"
  sh "$ROOT/scripts/check-version.sh"
  step "test-check-version.sh"
  # TODO(version): test-check-version.sh arrives with the version package; drop the guard once it is merged.
  if [ -f "$ROOT/scripts/test-check-version.sh" ]; then
    sh "$ROOT/scripts/test-check-version.sh"
  else
    echo "test-check-version.sh: not present yet (comes with the version package)"
  fi
  step "test-no-node-path.sh"
  sh "$ROOT/scripts/test-no-node-path.sh"
  node_guard
  step "test-node-guard.sh"
  sh "$ROOT/scripts/test-node-guard.sh"
fi

if [ "$STAGE" = all ]; then
  if [ "${CSTAN_LIVE:-}" = "1" ]; then
    step "live suites (CSTAN_LIVE=1)"
    # shellcheck disable=SC2086
    (cd "$ROOT/rust" && cargo test $JOBS --locked -p capstan-herdr -p capstan-daemon -p capstan-launcher \
      --test live --test live_agent --test prompt_relay_live --test researcher_live -- --ignored --test-threads=1)
  else
    echo
    echo "live suites: not run (set CSTAN_LIVE=1 to drive a real Herdr and Claude Code)"
  fi
fi

printf '\ncheck: %s passed\n' "$STAGE"
