#!/bin/sh
# Smoke test for install.sh. Builds the release assets (npm run release --no-dash --no-front: the npm tarball and the
# standalone binaries; the cstan-dash and cstan-front scenarios use fake programs, so cargo is not needed) and runs every install scenario against temp HOME / CAPSTAN_HOME /
# CAPSTAN_BIN_DIR dirs, for the npm tarball path (--no-binary / --tarball) and the binary path
# (--binary, or a release whose SHA256SUMS lists the binary). Release scenarios use file://, so curl.
# The build needs network for npm's dependency fetch and the Node archives of the binaries.
# Set CAPSTAN_TEST_RELEASE_DIR to a dir holding the release assets (capstan-controller-<v>.tgz,
# cstan-<v>-<os>-<arch> and SHA256SUMS) to skip the build. Exit code 0 means every scenario passed.
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

# Keep npm's cache from the real HOME so temp HOMEs stay warm.
REAL_CACHE="$(npm config get cache 2>/dev/null || true)"
[ -z "$REAL_CACHE" ] || export npm_config_cache="$REAL_CACHE"

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-install-test.XXXXXX")"
VERSION="$(node -p "require('$ROOT/package.json').version")"

# --- release artefacts -------------------------------------------------------
if [ -n "${CAPSTAN_TEST_RELEASE_DIR:-}" ]; then
  RELEASE_DIR="$CAPSTAN_TEST_RELEASE_DIR"
else
  RELEASE_DIR="$SANDBOX/release"
  (cd "$ROOT" && CSTAN_RELEASE_DIR="$RELEASE_DIR" npm run release -- --no-dash --no-front >"$SANDBOX/release.log" 2>&1) || {
    cat "$SANDBOX/release.log"
    echo "npm run release failed" >&2
    exit 1
  }
fi
TGZ="$RELEASE_DIR/capstan-controller-$VERSION.tgz"
SUMS="$RELEASE_DIR/SHA256SUMS"
[ -f "$TGZ" ] && [ -f "$SUMS" ] || {
  echo "missing $TGZ or $SUMS" >&2
  exit 1
}
# A release built with the front end and cstan-dash lists them too; the scenarios below that copy only the tarball and the
# binary use a SHA256SUMS without those entries (they have their own scenarios with fakes or the real files).
mkdir -p "$SANDBOX/base"
grep -v -e " cstan-dash-" -e " cstan-front-" "$SUMS" >"$SANDBOX/base/SHA256SUMS" || true
SUMS="$SANDBOX/base/SHA256SUMS"
SHA="$(awk -v a="capstan-controller-$VERSION.tgz" '{ n = $2; sub(/.*\//, "", n); if (n == a) print $1 }' "$SUMS")"

# new_env <name>: fresh temp HOME/CAPSTAN_HOME/CAPSTAN_BIN_DIR under the sandbox.
new_env() {
  # The previous scenario is done: drop its install and release copies, which hold a 130 MB binary each.
  [ -z "${E:-}" ] || rm -rf "$E"
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
for n in 8 25 60 120 200 300 400 $((TOTAL - 1)); do
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
check "tarball path never mentions fs-ext, a compiler or python" sh -c '! grep -qi "fs-ext\|node-gyp\|c++\|python" "$1"' _ "$E/out"

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
if run_install --no-binary --version "$VERSION" >"$E/out" 2>&1; then
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
check_not "tampered tarball is refused" run_install --no-binary --version "$VERSION"
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
if PATH="$E/stub:$PATH" run_install --no-binary >"$E/out" 2>&1; then
  pass "wget -S output with '[following]' resolves the latest version"
else
  fail "wget -S output with '[following]' resolves the latest version"
  cat "$E/out"
fi
check "wget-resolved install runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
check "no stray text in the resolved version" contains "$E/out" "Installed cstan $VERSION\$"

new_env badloc
STUB_LOCATION="https://github.com/akhilrs/capstan/releases/tag/v1.2.3-rc1" PATH="$E/stub:$PATH" \
  run_install --no-binary >"$E/out" 2>&1 && fail "pre-release latest tag is refused" || pass "pre-release latest tag is refused"
check_not "nothing installed for a bad latest tag" test -e "$CAPSTAN_HOME/current"
unset CAPSTAN_FETCHER CAPSTAN_RELEASE_BASE STUB_LOCATION STUB_ROOT
for bad in "0.1.1 [following]" "1.2" "1.2.3.4" "v1.2.3" "1.2.3-rc1" "1..3" ".1.2" "a.b.c" "1.2.3 "; do
  check_not "--version '$bad' is rejected" run_install --version "$bad" --tarball "$TGZ"
done
CAPSTAN_VERSION="1.2.3x"
export CAPSTAN_VERSION
check_not "CAPSTAN_VERSION=1.2.3x is rejected" run_install --tarball "$TGZ"
unset CAPSTAN_VERSION

# --- 9. tarball path: --version pin ------------------------------------------
new_env pin
check_not "tarball with a --version that does not match is refused" run_install --version 9.9.9 --tarball "$TGZ"
check_not "nothing installed for a mismatched pin" test -e "$CAPSTAN_HOME/current"
check "tarball with the matching --version installs" run_install --version "$VERSION" --tarball "$TGZ"

# --- 10. binary path ---------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) HOST_PLATFORM=linux-x64 ;;
  Linux-aarch64) HOST_PLATFORM=linux-arm64 ;;
  *) HOST_PLATFORM="" ;;
esac
BINNAME="cstan-$VERSION-$HOST_PLATFORM"
if [ -z "$HOST_PLATFORM" ] || [ ! -f "$RELEASE_DIR/$BINNAME" ]; then
  echo "skip binary scenarios: no $BINNAME in $RELEASE_DIR for this machine"
elif PATH=/usr/bin:/bin command -v node >/dev/null 2>&1; then
  echo "skip binary scenarios: node is in /usr/bin, so 'no node on PATH' cannot be shown"
else
  BIN="$RELEASE_DIR/$BINNAME"
  BINSHA="$(awk -v a="$BINNAME" '{ if ($2 == a) print $1 }' "$SUMS")"
  [ -n "$BINSHA" ] || fail "SHA256SUMS lists $BINNAME"
  # no_node_env: the install and the installed cstan run with no node on PATH.
  no_node_env() {
    new_env "$1"
    PATH="$CAPSTAN_BIN_DIR:/usr/bin:/bin"
    export PATH
  }

  no_node_env binary
  mkdir -p "$E/local"
  cp "$BIN" "$E/local/"
  grep "  $BINNAME\$" "$SUMS" >"$E/local/SHA256SUMS"
  check "node is not on the binary scenarios' PATH" sh -c '! command -v node'
  if run_install --binary "$E/local/$BINNAME" >"$E/out" 2>&1; then
    pass "binary install exits 0"
  else
    fail "binary install exits 0"
    cat "$E/out"
  fi
  check "binary install is verified against the SHA256SUMS next to it" contains "$E/out" "verified against SHA256SUMS"
  check "installed binary prints its version with no node" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "binary bin symlink resolves into CAPSTAN_HOME/current" sh -c '
    real="$(readlink -f "$CAPSTAN_BIN_DIR/cstan")"; case "$real" in "$(readlink -f "$CAPSTAN_HOME")"/current/*) exit 0 ;; esac; exit 1'
  check "binary install mentions no fs-ext or compiler" sh -c '! grep -qi "fs-ext\|node-gyp\|c++\|python" "$1"' _ "$E/out"
  check "re-install over the binary install exits 0" run_install --binary "$E/local/$BINNAME"
  check "exactly one installation after re-install" sh -c '
    [ "$(ls -A "$CAPSTAN_HOME" | tr "\n" " ")" = "current " ]'
  check "--sha256 alone verifies the binary" run_install --binary "$BIN" --sha256 "$BINSHA"
  check "--version matching the binary installs" run_install --binary "$BIN" --version "$VERSION"
  check_not "--version not matching the binary is refused" run_install --binary "$BIN" --version 9.9.9
  check_not "wrong --sha256 for the binary is refused" run_install --binary "$BIN" --sha256 "$BAD"
  check "previous binary install still works after a refusal" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  printf '%s  %s\n' "$BAD" "$BINNAME" >"$E/local/SHA256SUMS"
  check_not "binary that fails SHA256SUMS is refused" run_install --binary "$E/local/$BINNAME"
  printf '%s  other-file\n' "$BINSHA" >"$E/local/SHA256SUMS"
  check_not "SHA256SUMS without an entry for the binary is refused" run_install --binary "$E/local/$BINNAME"
  check_not "no staging dir left behind" sh -c '[ -n "$(ls -d "$CAPSTAN_HOME"/staging.* 2>/dev/null)" ]'
  check_not "--binary with --tarball is refused" run_install --binary "$BIN" --tarball "$TGZ"
  check "--uninstall removes the binary install" run_install --uninstall
  check_not "uninstall removes the binary symlink" test -e "$CAPSTAN_BIN_DIR/cstan"
  check_not "uninstall removes current (binary)" test -e "$CAPSTAN_HOME/current"

  # A release whose SHA256SUMS lists the binary installs the binary; --no-binary forces the tarball.
  no_node_env binrel
  BREL="$E/releases/download/v$VERSION"
  mkdir -p "$BREL"
  cp "$BIN" "$TGZ" "$SUMS" "$BREL/"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  if run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "release install picks the binary"
  else
    fail "release install picks the binary"
    cat "$E/out"
  fi
  check "release install says it installed the binary" contains "$E/out" "standalone binary"
  check "release-installed binary runs with no node" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  printf 'tamper' >>"$BREL/$BINNAME"
  check_not "tampered release binary is refused" run_install --version "$VERSION"
  check "the previous release install survives the refusal" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "--uninstall removes the release install" run_install --uninstall

  # --- the native front end (cstan-front) beside the binary -------------------------------------------------------------
  # Fake front ends stand in for the real one: a script that answers __front-version and otherwise hands over to
  # cstan-node beside it, which is all the installer relies on.
  FAKEFRONT="$SANDBOX/fake-front"
  FRONTNAME="cstan-front-$VERSION-$HOST_PLATFORM"
  mkdir -p "$FAKEFRONT/ok" "$FAKEFRONT/broken" "$FAKEFRONT/stale"
  cat >"$FAKEFRONT/ok/$FRONTNAME" <<FAKE
#!/bin/sh
if [ "\${1:-}" = __front-version ]; then echo "cstan-front $VERSION"; exit 0; fi
exec "\$(dirname "\$(readlink -f "\$0")")/cstan-node" "\$@"
FAKE
  printf '#!/bin/sh\nexit 1\n' >"$FAKEFRONT/broken/$FRONTNAME"
  printf '#!/bin/sh\necho "cstan-front 9.9.9"\n' >"$FAKEFRONT/stale/$FRONTNAME"
  chmod 755 "$FAKEFRONT"/*/"$FRONTNAME"
  sha_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
  }
  # front_release <env-name> <fake kind or none>: a fake release with the binary, the tarball and that front end.
  front_release() {
    no_node_env "$1"
    FREL="$E/releases/download/v$VERSION"
    mkdir -p "$FREL"
    ln -s "$BIN" "$FREL/$BINNAME"
    ln -s "$TGZ" "$FREL/capstan-controller-$VERSION.tgz"
    grep -e "  $BINNAME\$" -e "  capstan-controller-$VERSION.tgz\$" "$SUMS" >"$FREL/SHA256SUMS"
    if [ "$2" != none ]; then
      cp "$FAKEFRONT/$2/$FRONTNAME" "$FREL/"
      printf '%s  %s\n' "$(sha_of "$FREL/$FRONTNAME")" "$FRONTNAME" >>"$FREL/SHA256SUMS"
    fi
    export CAPSTAN_RELEASE_BASE="file://$E/releases"
  }
  same_file() { cmp -s "$1" "$2"; }

  front_release frontrel ok
  if run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "release with a front end installs"
  else
    fail "release with a front end installs"
    cat "$E/out"
  fi
  check "the front end is verified against SHA256SUMS" contains "$E/out" "front end checksum verified"
  check "bin/cstan is the front end" same_file "$CAPSTAN_HOME/current/bin/cstan" "$FAKEFRONT/ok/$FRONTNAME"
  check "bin/cstan-node is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan-node" "$BIN"
  check "the bin symlink points at current/bin/cstan" test "$(readlink "$CAPSTAN_BIN_DIR/cstan")" = "$CAPSTAN_HOME/current/bin/cstan"
  check "cstan --version runs through the front end" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "the success report names the front end" contains "$E/out" "front end: "
  check "no staging file is left in bin" test -z "$(ls "$CAPSTAN_HOME/current/bin" | grep -v '^cstan\(-node\)\?$' || true)"
  check "--uninstall removes both" run_install --uninstall
  check_not "uninstall leaves no current" test -e "$CAPSTAN_HOME/current"
  check_not "uninstall leaves no symlink" test -e "$CAPSTAN_BIN_DIR/cstan"

  # An upgrade from a v0.2.0-style install (the binary as bin/cstan) puts the front end in front of the binary.
  front_release frontup ok
  mkdir -p "$E/local"
  cp "$BIN" "$E/local/"
  check "a v0.2.0-style install (--no-front)" run_install --binary "$E/local/$BINNAME" --no-front
  check_not "that install has no cstan-node" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
  check "its bin/cstan is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan" "$BIN"
  if run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "upgrading from the binary-only layout exits 0"
  else
    fail "upgrading from the binary-only layout exits 0"
    cat "$E/out"
  fi
  check "after the upgrade bin/cstan is the front end" same_file "$CAPSTAN_HOME/current/bin/cstan" "$FAKEFRONT/ok/$FRONTNAME"
  check "after the upgrade bin/cstan-node is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan-node" "$BIN"
  check "the upgraded cstan runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "downgrading to the binary alone with --no-front" run_install --version "$VERSION" --no-front
  check_not "--no-front leaves no cstan-node" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
  check "--no-front bin/cstan is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan" "$BIN"
  check_not "--front-binary with --no-front is refused" run_install --version "$VERSION" --no-front --front-binary "$FAKEFRONT/ok/$FRONTNAME"

  # The real front end, when the release directory holds one (npm run build:cli -- --target <platform>).
  if [ -f "$RELEASE_DIR/$FRONTNAME" ]; then
    mkdir -p "$FAKEFRONT/real"
    cp "$RELEASE_DIR/$FRONTNAME" "$FAKEFRONT/real/"
    front_release frontreal real
    if run_install --version "$VERSION" >"$E/out" 2>&1; then
      pass "release with the real front end installs"
    else
      fail "release with the real front end installs"
      cat "$E/out"
    fi
    check "the real front end is bin/cstan" same_file "$CAPSTAN_HOME/current/bin/cstan" "$RELEASE_DIR/$FRONTNAME"
    check "the real front end's cstan-node is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan-node" "$BIN"
    check "the real front end prints its version" test "$("$CAPSTAN_HOME/current/bin/cstan" __front-version)" = "cstan-front $VERSION"
    check "cstan --version runs through the real front end" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
    check "--uninstall removes the real front-end install" run_install --uninstall
  else
    echo "skip real front end scenarios: no $FRONTNAME in $RELEASE_DIR"
  fi

  # A release without cstan-front installs the binary as bin/cstan, as before.
  front_release frontnone none
  if run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "release without a front end installs"
  else
    fail "release without a front end installs"
    cat "$E/out"
  fi
  check "the missing front end is noted" contains "$E/out" "no cstan front end for $HOST_PLATFORM"
  check "bin/cstan is the binary (no front end)" same_file "$CAPSTAN_HOME/current/bin/cstan" "$BIN"
  check_not "no cstan-node without a front end" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
  check "cstan runs without a front end" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"

  # A front end that does not run, or belongs to another release, falls back to the binary-only layout.
  for kind in broken stale; do
    front_release "front$kind" "$kind"
    if run_install --version "$VERSION" >"$E/out" 2>&1; then
      pass "a $kind front end does not fail the install"
    else
      fail "a $kind front end does not fail the install"
      cat "$E/out"
    fi
    check "a $kind front end is reported" contains "$E/out" "installed the binary alone"
    check "a $kind front end leaves the binary as bin/cstan" same_file "$CAPSTAN_HOME/current/bin/cstan" "$BIN"
    check_not "a $kind front end leaves no cstan-node" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
    check_not "a $kind front end leaves no staging file" test -e "$CAPSTAN_HOME/current/bin/cstan-front.new"
    check "cstan runs after a $kind front end" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  done

  # A tampered front end is refused and the previous install stays.
  front_release fronttamper ok
  check "install before tampering" run_install --version "$VERSION"
  printf 'tamper' >>"$FREL/$FRONTNAME"
  check_not "a tampered front end is refused" run_install --version "$VERSION"
  check "the previous install survives it" same_file "$CAPSTAN_HOME/current/bin/cstan-node" "$BIN"

  # --no-front and --front-binary.
  front_release frontopt ok
  check "--no-front installs the binary alone" run_install --version "$VERSION" --no-front
  check_not "--no-front leaves no cstan-node (release)" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
  mkdir -p "$E/local"
  cp "$BIN" "$FAKEFRONT/ok/$FRONTNAME" "$E/local/"
  printf '%s  %s\n' "$(sha_of "$E/local/$FRONTNAME")" "$FRONTNAME" >"$E/local/SHA256SUMS"
  grep "  $BINNAME\$" "$SUMS" >>"$E/local/SHA256SUMS"
  check "--front-binary installs the given front end" run_install --binary "$E/local/$BINNAME" --front-binary "$E/local/$FRONTNAME"
  check "--front-binary puts it at bin/cstan" same_file "$CAPSTAN_HOME/current/bin/cstan" "$FAKEFRONT/ok/$FRONTNAME"
  printf '%s  %s\n' "$BAD" "$FRONTNAME" >"$E/local/SHA256SUMS"
  check_not "--front-binary failing its SHA256SUMS is refused" run_install --binary "$E/local/$BINNAME" --front-binary "$E/local/$FRONTNAME"
  check_not "--front-binary with --tarball is refused" run_install --tarball "$TGZ" --front-binary "$E/local/$FRONTNAME"
  check "--uninstall removes the front-end install" run_install --uninstall
  check_not "uninstall removed cstan-node" test -e "$CAPSTAN_HOME/current/bin/cstan-node"

  # An installer from v0.2.0 against a release that has a front end still installs a working binary-only cstan.
  OLD_INSTALLER="$SANDBOX/install-v0.2.0.sh"
  if (cd "$ROOT" && git show v0.2.0:install.sh >"$OLD_INSTALLER" 2>/dev/null); then
    front_release frontold ok
    if sh "$OLD_INSTALLER" --version "$VERSION" </dev/null >"$E/out" 2>&1; then
      pass "the v0.2.0 installer against a release with a front end exits 0"
    else
      fail "the v0.2.0 installer against a release with a front end exits 0"
      cat "$E/out"
    fi
    check "the old installer's bin/cstan is the binary" same_file "$CAPSTAN_HOME/current/bin/cstan" "$BIN"
    check "the old installer's cstan runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  else
    echo "skip old-installer scenario: tag v0.2.0 is not in this clone"
  fi
  unset CAPSTAN_RELEASE_BASE

  # A release without a binary for this machine keeps the tarball path (it needs node).
  new_env nobin
  NREL="$E/releases/download/v$VERSION"
  mkdir -p "$NREL"
  cp "$TGZ" "$NREL/"
  printf '%s  capstan-controller-%s.tgz\n' "$SHA" "$VERSION" >"$NREL/SHA256SUMS"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  if run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "release without a binary installs the tarball"
  else
    fail "release without a binary installs the tarball"
    cat "$E/out"
  fi
  check "fallback to the tarball is announced" contains "$E/out" "using the npm tarball"
  unset CAPSTAN_RELEASE_BASE
fi

# --- 11. cstan-dash (the Rust dashboard) ----------------------------------------
# Fake dashboards stand in for the real binary: a script that answers --version is enough for the installer.
if [ -z "$HOST_PLATFORM" ]; then
  echo "skip cstan-dash scenarios: no release platform for this machine"
else
  DASHNAME="cstan-dash-$VERSION-$HOST_PLATFORM"
  FAKEDASH="$SANDBOX/fake-dash"
  mkdir -p "$FAKEDASH"
  printf '#!/bin/sh\necho "cstan-dash %s"\n' "$VERSION" >"$FAKEDASH/$DASHNAME"
  chmod 755 "$FAKEDASH/$DASHNAME"
  if command -v sha256sum >/dev/null 2>&1; then
    DASHSHA="$(sha256sum "$FAKEDASH/$DASHNAME" | cut -d ' ' -f 1)"
  else
    DASHSHA="$(shasum -a 256 "$FAKEDASH/$DASHNAME" | cut -d ' ' -f 1)"
  fi
  # dash_release <env-name> <with-dash>: a fake release holding the tarball and optionally the dashboard.
  dash_release() {
    new_env "$1"
    DREL="$E/releases/download/v$VERSION"
    mkdir -p "$DREL"
    cp "$TGZ" "$DREL/"
    printf '%s  capstan-controller-%s.tgz\n' "$SHA" "$VERSION" >"$DREL/SHA256SUMS"
    if [ "$2" = yes ]; then
      cp "$FAKEDASH/$DASHNAME" "$DREL/"
      printf '%s  %s\n' "$DASHSHA" "$DASHNAME" >>"$DREL/SHA256SUMS"
    fi
    export CAPSTAN_RELEASE_BASE="file://$E/releases"
  }

  dash_release dashrel yes
  if run_install --no-binary --version "$VERSION" >"$E/out" 2>&1; then
    pass "release with cstan-dash installs"
  else
    fail "release with cstan-dash installs"
    cat "$E/out"
  fi
  check "cstan-dash is verified against SHA256SUMS" contains "$E/out" "cstan-dash checksum verified"
  check "cstan-dash is installed beside cstan" test -x "$CAPSTAN_HOME/current/bin/cstan-dash"
  check "installed cstan-dash runs" test "$("$CAPSTAN_HOME/current/bin/cstan-dash" --version)" = "cstan-dash $VERSION"
  check "the success report names the dashboard" contains "$E/out" "cstan-dash (cstan dash uses it)"

  printf 'tamper' >>"$DREL/$DASHNAME"
  check_not "a cstan-dash with a wrong sha is refused" run_install --no-binary --version "$VERSION"
  check "the previous install survives a refused cstan-dash" test -x "$CAPSTAN_HOME/current/bin/cstan-dash"
  check "no staging dir left after a refused cstan-dash" sh -c '[ -z "$(ls -d "$CAPSTAN_HOME"/staging.* 2>/dev/null)" ]'

  dash_release dashno yes
  check "--no-dash installs cstan alone" run_install --no-binary --no-dash --version "$VERSION"
  check_not "--no-dash leaves no cstan-dash" test -e "$CAPSTAN_HOME/current/bin/cstan-dash"
  check "cstan still runs without cstan-dash" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"

  dash_release dashmissing no
  if run_install --no-binary --version "$VERSION" >"$E/out" 2>&1; then
    pass "release without cstan-dash installs"
  else
    fail "release without cstan-dash installs"
    cat "$E/out"
  fi
  check "the missing cstan-dash is noted" contains "$E/out" "has no cstan-dash for $HOST_PLATFORM"
  check_not "no cstan-dash installed for such a release" test -e "$CAPSTAN_HOME/current/bin/cstan-dash"

  new_env dashlocal
  unset CAPSTAN_RELEASE_BASE
  check "--dash-binary installs the given file" run_install --tarball "$TGZ" --sha256 "$SHA" --dash-binary "$FAKEDASH/$DASHNAME"
  check "--dash-binary lands beside cstan" test -x "$CAPSTAN_HOME/current/bin/cstan-dash"
  printf '%s  %s\n' "$BAD" "$DASHNAME" >"$FAKEDASH/SHA256SUMS"
  check_not "--dash-binary failing its SHA256SUMS is refused" run_install --tarball "$TGZ" --sha256 "$SHA" --dash-binary "$FAKEDASH/$DASHNAME"
  check_not "--dash-binary with --no-dash is refused" run_install --tarball "$TGZ" --sha256 "$SHA" --dash-binary "$FAKEDASH/$DASHNAME" --no-dash
  unset CAPSTAN_RELEASE_BASE
fi

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "all install scenarios passed"
  exit 0
fi
echo "$FAILURES check(s) failed"
exit 1
