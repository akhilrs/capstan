#!/bin/sh
# Smoke test for install.sh. Builds the release tarball (npm run release) and runs
# every install scenario against temp HOME / CAPSTAN_HOME / CAPSTAN_BIN_DIR dirs.
# Needs no network beyond npm's dependency fetch, but offline a warm npm cache is
# not enough: node-gyp also needs cached Node headers (~/.cache/node-gyp or
# npm_config_nodedir) and a supported python3. Release scenarios use file://, so curl.
# Set CAPSTAN_TEST_RELEASE_DIR to a dir holding capstan-controller-<v>.tgz and
# SHA256SUMS to skip the build. Exit code 0 means every scenario passed.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="$ROOT/install.sh"
FAILURES=0
SANDBOX=""

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}

# check <description> <command...>: passes when the command succeeds.
check() {
  desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}

# check_not <description> <command...>: passes when the command fails.
check_not() {
  desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then fail "$desc"; else pass "$desc"; fi
}

cleanup() {
  [ -z "$SANDBOX" ] || rm -rf "$SANDBOX"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

contains() { grep -q -- "$2" "$1"; }

# Keep npm's cache and node-gyp headers from the real HOME so temp HOMEs stay warm.
REAL_CACHE="$(npm config get cache 2>/dev/null || true)"
[ -z "$REAL_CACHE" ] || export npm_config_cache="$REAL_CACHE"
if [ -d "${HOME:-/nonexistent}/.cache/node-gyp" ]; then
  export npm_config_devdir="$HOME/.cache/node-gyp"
fi

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-install-test.XXXXXX")"
VERSION="$(node -p "require('$ROOT/package.json').version")"

# --- release artefacts -------------------------------------------------------
if [ -n "${CAPSTAN_TEST_RELEASE_DIR:-}" ]; then
  RELEASE_DIR="$CAPSTAN_TEST_RELEASE_DIR"
else
  (cd "$ROOT" && npm run release >"$SANDBOX/release.log" 2>&1) || {
    cat "$SANDBOX/release.log"
    echo "npm run release failed" >&2
    exit 1
  }
  RELEASE_DIR="$ROOT/release"
fi
TGZ="$RELEASE_DIR/capstan-controller-$VERSION.tgz"
SUMS="$RELEASE_DIR/SHA256SUMS"
[ -f "$TGZ" ] && [ -f "$SUMS" ] || {
  echo "missing $TGZ or $SUMS" >&2
  exit 1
}
SHA="$(awk -v a="capstan-controller-$VERSION.tgz" '{ n = $2; sub(/.*\//, "", n); if (n == a) print $1 }' "$SUMS")"

# new_env <name>: fresh temp HOME/CAPSTAN_HOME/CAPSTAN_BIN_DIR under the sandbox.
new_env() {
  E="$SANDBOX/$1"
  mkdir -p "$E/home"
  export HOME="$E/home" CAPSTAN_HOME="$E/share" CAPSTAN_BIN_DIR="$E/bin"
  unset XDG_DATA_HOME CAPSTAN_VERSION CAPSTAN_TARBALL CAPSTAN_SHA256 CAPSTAN_RELEASE_BASE
  PATH="$CAPSTAN_BIN_DIR:$BASE_PATH"
  export PATH
  git config --global user.name test >/dev/null 2>&1 || true
}
BASE_PATH="$PATH"

run_install() {
  sh "$INSTALLER" "$@" </dev/null
}

# --- 1. syntax ---------------------------------------------------------------
check "sh -n install.sh" sh -n "$INSTALLER"
check "sh -n test-install.sh" sh -n "$ROOT/scripts/test-install.sh"
if command -v dash >/dev/null 2>&1; then
  check "dash -n install.sh" dash -n "$INSTALLER"
else
  echo "skip dash -n: dash not installed"
fi
if command -v shellcheck >/dev/null 2>&1; then
  check "shellcheck -s sh" shellcheck -s sh "$INSTALLER" "$ROOT/scripts/test-install.sh"
else
  echo "skip shellcheck: not installed"
fi

# --- 2. truncation safety ----------------------------------------------------
new_env trunc
TOTAL="$(wc -l <"$INSTALLER" | tr -d ' ')"
for n in 8 25 60 120 200 $((TOTAL - 1)); do
  [ "$n" -lt "$TOTAL" ] || continue
  head -n "$n" "$INSTALLER" | sh >"$E/trunc.out" 2>&1 || true
  if [ ! -e "$CAPSTAN_HOME" ] && [ ! -e "$CAPSTAN_BIN_DIR" ] && ! contains "$E/trunc.out" "Installing\|Downloading\|Uninstalling"; then
    pass "truncated at line $n runs nothing"
  else
    fail "truncated at line $n ran something"
  fi
done

# --- 3. fresh install from the local tarball ---------------------------------
new_env fresh
if run_install --tarball "$TGZ" --sha256 "$SHA" >"$E/out" 2>&1; then
  pass "fresh install exits 0"
else
  fail "fresh install exits 0"
  cat "$E/out"
fi
check "cstan --version is 'cstan $VERSION'" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
check "cstan --help exits 0" cstan --help
check "bin symlink resolves into CAPSTAN_HOME/current" sh -c '
  real="$(readlink -f "$CAPSTAN_BIN_DIR/cstan")"; case "$real" in "$(readlink -f "$CAPSTAN_HOME")"/current/*) exit 0 ;; esac; exit 1'
mkdir -p "$E/repo"
(cd "$E/repo" && git init -q . && cstan init >"$E/init.out" 2>&1) || cat "$E/init.out"
check "cstan init creates .capstan/" test -d "$E/repo/.capstan"

# --- 4. re-run, wrong checksum -----------------------------------------------
check "re-run exits 0" run_install --tarball "$TGZ" --sha256 "$SHA"
check "exactly one installation after re-run" sh -c '
  [ "$(ls -A "$CAPSTAN_HOME" | tr "\n" " ")" = "current " ]'
BAD="0000000000000000000000000000000000000000000000000000000000000000"
check_not "wrong --sha256 exits non-zero" run_install --tarball "$TGZ" --sha256 "$BAD"
check "previous install still works after failure" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
check "no staging dir left behind" sh -c '[ -z "$(ls -d "$CAPSTAN_HOME"/staging.* 2>/dev/null)" ]'

# --- 5. wrong Node, missing tools --------------------------------------------
new_env node22
mkdir -p "$E/fake"
printf '#!/bin/sh\necho v22.0.0\n' >"$E/fake/node"
chmod +x "$E/fake/node"
if PATH="$E/fake:$PATH" run_install --version "$VERSION" --tarball "$TGZ" >"$E/out" 2>&1; then
  fail "node 22 install exits non-zero"
else
  pass "node 22 install exits non-zero"
fi
check "message names Node 24" contains "$E/out" "Node 24"
check_not "no download before the Node check" contains "$E/out" "Downloading"
check_not "nothing installed under node 22" test -e "$CAPSTAN_HOME"

new_env warn
run_install --tarball "$TGZ" --sha256 "$SHA" >"$E/out" 2>&1 || fail "install with missing herdr/claude exits 0"
if ! command -v herdr >/dev/null 2>&1; then check "missing herdr only warns" contains "$E/out" "herdr not found"; fi
if ! command -v claude >/dev/null 2>&1; then check "missing claude only warns" contains "$E/out" "claude not found"; fi

# --- 6. uninstall ------------------------------------------------------------
new_env uninstall
run_install --tarball "$TGZ" --sha256 "$SHA" >/dev/null 2>&1 || fail "install for uninstall scenario"
mkdir -p "$E/repo/.capstan"
check "--uninstall exits 0" run_install --uninstall
check_not "uninstall removes the bin symlink" test -e "$CAPSTAN_BIN_DIR/cstan"
check_not "uninstall removes current" test -e "$CAPSTAN_HOME/current"
check "uninstall leaves project .capstan/ alone" test -d "$E/repo/.capstan"
check "second --uninstall exits 0" run_install --uninstall
mkdir -p "$CAPSTAN_BIN_DIR" "$E/elsewhere"
ln -s "$E/elsewhere" "$CAPSTAN_BIN_DIR/cstan"
check "--uninstall with a foreign symlink exits 0" run_install --uninstall
check "foreign cstan symlink is left alone" test -L "$CAPSTAN_BIN_DIR/cstan"

# --- 7. release mode against a fake release ----------------------------------
new_env release
REL="$E/releases/download/v$VERSION"
mkdir -p "$REL"
cp "$TGZ" "$SUMS" "$REL/"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
if run_install --version "$VERSION" >"$E/out" 2>&1; then
  pass "release install exits 0"
else
  fail "release install exits 0"
  cat "$E/out"
fi
check "release install is verified" contains "$E/out" "verified against SHA256SUMS"
check "release install runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"

new_env tamper
TREL="$E/releases/download/v$VERSION"
mkdir -p "$TREL"
cp "$TGZ" "$SUMS" "$TREL/"
printf 'tamper' >>"$TREL/capstan-controller-$VERSION.tgz"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
check_not "tampered tarball is refused" run_install --version "$VERSION"
check_not "nothing installed from a tampered tarball" test -e "$CAPSTAN_HOME/current"

# --- 8. version parsing: wget -S output and bad versions -----------------------
new_env wget
mkdir -p "$E/stub"
cat >"$E/stub/wget" <<'STUB'
#!/bin/sh
# stub wget: canned redirect for -S, file copies from $STUB_ROOT otherwise
out=""; url=""; sflag=0
while [ $# -gt 0 ]; do
  case "$1" in
    -S) sflag=1 ;;
    -O) out="$2"; shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
if [ $sflag = 1 ]; then
  printf '  HTTP/1.1 302 Found\n  Location: %s\nLocation: %s [following]\n0 redirections exceeded.\n' "$STUB_LOCATION" "$STUB_LOCATION" >&2
  exit 8
fi
cp "$STUB_ROOT/${url##*/}" "$out"
STUB
chmod +x "$E/stub/wget"
mkdir -p "$E/stubroot"
cp "$TGZ" "$SUMS" "$E/stubroot/"
export CAPSTAN_FETCHER=wget STUB_ROOT="$E/stubroot"
export CAPSTAN_RELEASE_BASE="http://stub.invalid/releases"
STUB_LOCATION="https://github.com/akhilrs/capstan/releases/tag/v$VERSION"
export STUB_LOCATION
if PATH="$E/stub:$PATH" run_install >"$E/out" 2>&1; then
  pass "wget -S output with '[following]' resolves the latest version"
else
  fail "wget -S output with '[following]' resolves the latest version"
  cat "$E/out"
fi
check "wget-resolved install runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
check "no stray text in the resolved version" contains "$E/out" "Installed cstan $VERSION\$"

new_env badloc
STUB_LOCATION="https://github.com/akhilrs/capstan/releases/tag/v1.2.3-rc1" PATH="$E/stub:$PATH" \
  run_install >"$E/out" 2>&1 && fail "pre-release latest tag is refused" || pass "pre-release latest tag is refused"
check_not "nothing installed for a bad latest tag" test -e "$CAPSTAN_HOME/current"
unset CAPSTAN_FETCHER CAPSTAN_RELEASE_BASE STUB_LOCATION STUB_ROOT
for bad in "0.1.1 [following]" "1.2" "1.2.3.4" "v1.2.3" "1.2.3-rc1" "1..3" ".1.2" "a.b.c" "1.2.3 "; do
  check_not "--version '$bad' is rejected" run_install --version "$bad" --tarball "$TGZ"
done
CAPSTAN_VERSION="1.2.3x"
export CAPSTAN_VERSION
check_not "CAPSTAN_VERSION=1.2.3x is rejected" run_install --tarball "$TGZ"
unset CAPSTAN_VERSION

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "all install scenarios passed"
  exit 0
fi
echo "$FAILURES check(s) failed"
exit 1
