#!/bin/sh
# The single gate. Needs no node, npm or python on PATH: cargo, git and a POSIX shell.
#   - cargo fmt --check, clippy -D warnings and cargo test --locked for rust/ (the workspace), dash/ and tools/release
#   - the release builds of cstan and cstan-dash, then scripts/smoke-binary.sh on them
#   - scripts/test-install.sh, scripts/check-release-workflow.sh, scripts/check-version.sh
#   - the live suites (a real Herdr, and a real Claude Code where a test needs one), only with CSTAN_LIVE=1
#   - the test-map check: every `git ls-files 'test/*.test.ts' 'test/*.test.tsx'` file is classified by docs/test-map/core.md,
#     live.md or release.md, no row names a file that is gone or is still marked `cutover`, and live.md has no `Not ported`
#     row and no Deferred section
# Usage: scripts/check.sh [stage]    no argument runs every stage, in this order; CI runs one job per stage
#   rust       fmt, clippy and test of the rust/ workspace
#   dash       fmt, clippy and test of dash/
#   tools      fmt, clippy and test of tools/release
#   scripts    smoke-binary.sh, test-install.sh, check-release-workflow.sh, check-version.sh, test-check-version.sh,
#              test-no-node-path.sh and the test-map check
#   release    the release builds of cstan and cstan-dash (also run by "all" when CSTAN_RELEASE_BUILD=1; the Release
#              workflow builds and verifies the real binaries on tags, so CI skips them)
#   test-map   only the test-map check
#   (--test-map is kept as an alias of test-map)
# Environment: CSTAN_RELEASE_BUILD=1 (build the release binaries before the scripts stage so smoke-binary.sh runs on them),
#   CSTAN_NO_NODE=1 (before any stage, drop node, npm, npx, corepack and tsc from PATH and fail if one still resolves),
#   CARGO_TARGET_DIR (honoured by cargo; point it outside the checkout on a shared machine), CSTAN_CHECK_JOBS
#   (cargo -j; cargo's own default when unset), CSTAN_LIVE=1.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAP="$ROOT/docs/test-map"
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
  (cd "$ROOT/$dir" && cargo test $JOBS --locked)
)

# ---------------------------------------------------------------------------------------------------- the test-map check

test_map_check() {
  step "test map"
  bad=0
  for file in core live release; do
    [ -f "$MAP/$file.md" ] || { echo "test-map: $MAP/$file.md is missing" >&2; bad=1; }
  done
  [ "$bad" = 0 ] || return 1
  tests="$(cd "$ROOT" && git ls-files 'test/*.test.ts' 'test/*.test.tsx')"
  [ -n "$tests" ] || { echo "test-map: git ls-files found no test/*.test.ts file" >&2; return 1; }
  # Every test file is named, in backticks, by one of the three maps.
  for path in $tests; do
    name="${path#test/}"
    if ! grep -qF -e "\`$name\`" -e "\`$path\`" "$MAP/core.md" "$MAP/live.md" "$MAP/release.md"; then
      echo "test-map: $name is classified by none of core.md, live.md and release.md" >&2
      bad=1
    fi
  done
  # A table row may only name test files that exist.
  names="$(printf '%s\n' "$tests" | sed 's|^test/||')"
  for map in core live release; do
    # shellcheck disable=SC2016 # the backticks are literal: the maps name files in backticks
    grep -h '^|' "$MAP/$map.md" | grep -o '`\(test/\)\{0,1\}[A-Za-z0-9_.-]*\.test\.tsx\{0,1\}`' | tr -d '`' | sed 's|^test/||' | sort -u |
      while IFS= read -r named; do
        printf '%s\n' "$names" | grep -qxF -- "$named" || echo "test-map: $map.md has a row naming $named, which is not a file of test/"
      done
  done >"${TMPDIR:-/tmp}/capstan-testmap.$$"
  if [ -s "${TMPDIR:-/tmp}/capstan-testmap.$$" ]; then
    cat "${TMPDIR:-/tmp}/capstan-testmap.$$" >&2
    bad=1
  fi
  rm -f "${TMPDIR:-/tmp}/capstan-testmap.$$"
  # No row is still waiting for cutover, and live.md has nothing left to port.
  if grep -h '^|' "$MAP/core.md" "$MAP/live.md" "$MAP/release.md" | grep -qi 'cutover'; then
    echo "test-map: a row is still marked cutover:" >&2
    grep -hn '^|' "$MAP/core.md" "$MAP/live.md" "$MAP/release.md" | grep -i 'cutover' | cut -c1-160 >&2
    bad=1
  fi
  if grep '^|' "$MAP/live.md" | grep -qi 'not ported'; then
    echo "test-map: live.md still has a Not ported row" >&2
    bad=1
  fi
  if grep -qi '^#\{1,6\} .*deferred' "$MAP/live.md"; then
    echo "test-map: live.md still has a Deferred section" >&2
    bad=1
  fi
  [ "$bad" = 0 ] || return 1
  count="$(printf '%s\n' "$tests" | wc -l | tr -d ' ')"
  echo "test-map: $count test files classified"
}

STAGE="${1:-all}"
case "$STAGE" in
  --test-map) STAGE=test-map ;;
  all | rust | dash | tools | scripts | release | test-map) ;;
  *) echo "usage: scripts/check.sh [rust|dash|tools|scripts|release|test-map]" >&2; exit 2 ;;
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

if [ "$STAGE" = test-map ]; then
  test_map_check
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

if want scripts; then
  test_map_check
fi
printf '\ncheck: %s passed\n' "$STAGE"
