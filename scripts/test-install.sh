#!/bin/sh
# Smoke test for install.sh. Fabricates releases (shell scripts standing in for the Rust binaries, with a real
# SHA256SUMS) and runs every install scenario against temp HOME / CAPSTAN_HOME / CAPSTAN_BIN_DIR dirs:
# a fresh install, platform detection, checksum refusals, the 0.4.0 cutoff, and the upgrade from each earlier layout
# (the Node SEA binary, the front end with cstan-node, the npm global prefix), whose fabricated installs follow what the
# v0.2.0 and v0.3.0 install.sh wrote. Release scenarios use file:// bases, so curl; `latest` is a stub in front of curl.
# It needs no network and no node: node directories are dropped from PATH for the whole run.
# Set CAPSTAN_TEST_RELEASE_DIR to a directory holding a real release (cstan-<v>-linux-<arch>,
# cstan-dash-<v>-linux-<arch> and SHA256SUMS) to also install it for this machine. Exit code 0 means all passed.
# SC2016 is disabled for the whole file: the sh -c scripts and printf formats below are deliberately single-quoted,
# so that "$1", "${1:-}" and the like are expanded by the inner shell or the generated stub, not by this script.
# shellcheck disable=SC2016
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="$ROOT/install.sh"
FAILURES=0
SANDBOX=""
SIM_PIDS=""

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

# check_shellcheck: like check, but prints shellcheck's findings when it fails (CI logs otherwise hide them).
check_shellcheck() {
  if out="$(shellcheck -s sh "$@" 2>&1)"; then
    pass "shellcheck -s sh"
  else
    fail "shellcheck -s sh"
    printf '%s\n' "$out"
  fi
}

# check_not <description> <command...>: passes when the command fails.
check_not() {
  desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then fail "$desc"; else pass "$desc"; fi
}

cleanup() {
  for pid in $SIM_PIDS; do kill "$pid" 2>/dev/null || true; done
  [ -z "$SANDBOX" ] || rm -rf "$SANDBOX"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

contains() { grep -q -- "$2" "$1"; }

# No node on PATH, for the installer and for everything it runs.
CLEAN_PATH=""
OLD_IFS="$IFS"
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  [ ! -x "$dir/node" ] || continue
  CLEAN_PATH="${CLEAN_PATH:+$CLEAN_PATH:}$dir"
done
IFS="$OLD_IFS"
PATH="$CLEAN_PATH"
export PATH
if command -v node >/dev/null 2>&1; then
  echo "node is still on PATH ($(command -v node)); the no-node run cannot be shown" >&2
  exit 1
fi
REAL_CURL="$(command -v curl || true)"
[ -n "$REAL_CURL" ] || { echo "curl is required for the file:// release scenarios" >&2; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-test-install.XXXXXX")"
VERSION=0.4.0
OLD_VERSION=0.3.0
BASE_PATH="$PATH"
RAW="https://raw.githubusercontent.com/akhilrs/capstan"

# make_release <root> <version> [binary version]: <root>/download/v<version>/ with both archs of cstan and cstan-dash
# (shell scripts that print their version; the first line after the shebang names the asset) and SHA256SUMS.
make_release() {
  rel="$1/download/v$2"
  shown="${3:-$2}"
  mkdir -p "$rel"
  for arch in x64 arm64; do
    for prog in cstan cstan-dash; do
      name="$prog-$2-linux-$arch"
      printf '#!/bin/sh\n# %s\ncase "${1:-}" in --version) echo "%s %s" ;; *) echo "%s ran" ;; esac\n' "$name" "$prog" "$shown" "$name" >"$rel/$name"
      chmod 755 "$rel/$name"
    done
  done
  (cd "$rel" && sha256sum cstan-* >SHA256SUMS)
}

# new_env <name>: fresh temp HOME/CAPSTAN_HOME/CAPSTAN_BIN_DIR under the sandbox.
new_env() {
  [ -z "${E:-}" ] || rm -rf "$E"
  E="$SANDBOX/$1"
  mkdir -p "$E/home" "$E/stub"
  export HOME="$E/home" CAPSTAN_HOME="$E/share" CAPSTAN_BIN_DIR="$E/bin"
  unset XDG_DATA_HOME CAPSTAN_VERSION CAPSTAN_BINARY CAPSTAN_SHA256 CAPSTAN_RELEASE_BASE CAPSTAN_FETCHER
  PATH="$CAPSTAN_BIN_DIR:$BASE_PATH"
  export PATH
  git config --global user.name test >/dev/null 2>&1 || true
}

run_install() {
  sh "$INSTALLER" "$@" </dev/null
}

# stub_uname <os> <machine>: a uname in front of PATH.
stub_uname() {
  cat >"$E/stub/uname" <<STUB
#!/bin/sh
case "\${1:-}" in
  -s) echo "$1" ;;
  -m) echo "$2" ;;
  *) exec /usr/bin/uname "\$@" ;;
esac
STUB
  chmod +x "$E/stub/uname"
}

# stub_latest <version>: a curl in front of PATH that answers the releases/latest lookup with that tag.
stub_latest() {
  cat >"$E/stub/curl" <<STUB
#!/bin/sh
case " \$* " in
  *url_effective*)
    for last; do :; done
    case "\$last" in */latest) printf '%s' "\$CAPSTAN_RELEASE_BASE/tag/v$1"; exit 0 ;; esac
    ;;
esac
exec "$REAL_CURL" "\$@"
STUB
  chmod +x "$E/stub/curl"
}

installed_files() {
  (cd "$CAPSTAN_HOME/current" && find . -mindepth 1 | sort | tr '\n' ' ')
}

# --- 1. syntax -----------------------------------------------------------------
check "sh -n install.sh" sh -n "$INSTALLER"
check "sh -n test-install.sh" sh -n "$ROOT/scripts/test-install.sh"
if command -v dash >/dev/null 2>&1; then
  check "dash -n install.sh" dash -n "$INSTALLER"
else
  echo "skip dash -n: dash not installed"
fi
if command -v shellcheck >/dev/null 2>&1; then
  check_shellcheck "$INSTALLER" "$ROOT/scripts/test-install.sh"
else
  echo "skip shellcheck: not installed"
fi
check "install.sh keeps FIRST_RUST_VERSION=0.4.0" grep -q '^FIRST_RUST_VERSION=0.4.0$' "$INSTALLER"
check_not "install.sh has no npm, tarball or node step" grep -q -i -E 'npm install|tarball|check_node|NODE_MAJOR' "$INSTALLER"

# --- 2. truncation safety --------------------------------------------------------
new_env trunc
TOTAL="$(wc -l <"$INSTALLER" | tr -d ' ')"
for n in 8 25 60 120 200 300 $((TOTAL - 1)); do
  [ "$n" -lt "$TOTAL" ] || continue
  head -n "$n" "$INSTALLER" | sh >"$E/trunc.out" 2>&1 || true
  if [ ! -e "$CAPSTAN_HOME" ] && [ ! -e "$CAPSTAN_BIN_DIR" ] && ! contains "$E/trunc.out" "Installing\|Downloading\|Uninstalling"; then
    pass "truncated at line $n runs nothing"
  else
    fail "truncated at line $n ran something"
  fi
done

# --- 3. fresh install from a release ------------------------------------------
new_env fresh
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
case "$(uname -m)" in
  x86_64 | amd64) HOST_ARCH=x64 ;;
  aarch64 | arm64) HOST_ARCH=arm64 ;;
  *) HOST_ARCH="" ;;
esac
if [ -z "$HOST_ARCH" ]; then
  echo "skip fresh install on this CPU: $(uname -m)"
else
  if run_install --version "$VERSION" >"$E/out" 2>&1; then pass "fresh install exits 0"; else fail "fresh install exits 0"; cat "$E/out"; fi
  check "cstan --version is 'cstan $VERSION'" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "cstan-dash --version is 'cstan-dash $VERSION'" test "$("$CAPSTAN_HOME/current/bin/cstan-dash" --version 2>/dev/null)" = "cstan-dash $VERSION"
  check "the install holds bin/cstan and bin/cstan-dash only" test "$(installed_files)" = "./bin ./bin/cstan ./bin/cstan-dash "
  check "the checksum was verified against SHA256SUMS" contains "$E/out" "Checksum verified against SHA256SUMS"
  check "bin symlink resolves into CAPSTAN_HOME/current" sh -c '
    real="$(readlink -f "$CAPSTAN_BIN_DIR/cstan")"; case "$real" in "$(readlink -f "$CAPSTAN_HOME")"/current/*) exit 0 ;; esac; exit 1'
  check "no node on PATH during the install" sh -c '! command -v node'
  check "re-run exits 0" run_install --version "$VERSION"
  check "re-run says it is already installed" sh -c 'sh "$1" --version "$2" </dev/null 2>&1 | grep -q "already installed"' _ "$INSTALLER" "$VERSION"
  check "exactly one installation after re-run" sh -c '[ "$(ls -A "$CAPSTAN_HOME" | tr "\n" " ")" = "current " ]'
  check "no staging dir left behind" sh -c '[ -z "$(ls -d "$CAPSTAN_HOME"/staging.* 2>/dev/null)" ]'
  check "missing herdr only warns" sh -c 'command -v herdr >/dev/null 2>&1 || grep -q "herdr not found" "$1"' _ "$E/out"
  check "the success text says to run cstan stop && cstan start" contains "$E/out" "cstan stop && cstan start"
fi

# --- 4. platform detection ---------------------------------------------------------
for case_ in "x86_64:x64" "amd64:x64" "aarch64:arm64" "arm64:arm64"; do
  machine="${case_%%:*}"
  want="${case_#*:}"
  new_env "plat-$machine"
  make_release "$E/releases" "$VERSION"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  stub_uname Linux "$machine"
  if PATH="$E/stub:$PATH" run_install --version "$VERSION" >"$E/out" 2>&1; then
    pass "uname -m $machine installs"
  else
    fail "uname -m $machine installs"
    cat "$E/out"
  fi
  check "uname -m $machine takes the linux-$want cstan" cmp -s "$CAPSTAN_HOME/current/bin/cstan" "$E/releases/download/v$VERSION/cstan-$VERSION-linux-$want"
  check "uname -m $machine takes the linux-$want cstan-dash" cmp -s "$CAPSTAN_HOME/current/bin/cstan-dash" "$E/releases/download/v$VERSION/cstan-dash-$VERSION-linux-$want"
done
new_env plat-riscv
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
stub_uname Linux riscv64
if PATH="$E/stub:$PATH" run_install --version "$VERSION" >"$E/out" 2>&1; then
  fail "an unsupported CPU is refused"
else
  pass "an unsupported CPU is refused"
fi
check "the refusal names the CPU" contains "$E/out" "riscv64"
check_not "nothing installed for an unsupported CPU" test -e "$CAPSTAN_HOME/current"
new_env plat-darwin
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
stub_uname Darwin arm64
if PATH="$E/stub:$PATH" run_install --version "$VERSION" >"$E/out" 2>&1; then
  fail "macOS is refused"
else
  pass "macOS is refused"
fi
check_not "nothing installed on macOS" test -e "$CAPSTAN_HOME/current"

# --- 5. checksum refusals --------------------------------------------------------------
new_env sums
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
REL="$E/releases/download/v$VERSION"
run_install --version "$VERSION" >/dev/null 2>&1 || fail "install for the checksum scenarios"
GOOD="$(cstan --version)"
for prog in cstan cstan-dash; do
  for arch in x64 arm64; do
    cp "$REL/$prog-$VERSION-linux-$arch" "$E/keep-$prog-$arch"
    printf 'tamper' >>"$REL/$prog-$VERSION-linux-$arch"
  done
  PLATFORM_ASSET="$prog-$VERSION-linux-$HOST_ARCH"
  if run_install --version "$VERSION" >"$E/out" 2>&1; then fail "a tampered $prog is refused"; else pass "a tampered $prog is refused"; fi
  check "the refusal for $prog says sha256 mismatch" contains "$E/out" "sha256 mismatch for $PLATFORM_ASSET"
  check "the previous install still works after the $prog refusal" test "$(cstan --version 2>/dev/null)" = "$GOOD"
  check "no staging dir left after the $prog refusal" sh -c '[ -z "$(ls -d "$CAPSTAN_HOME"/staging.* 2>/dev/null)" ]'
  for arch in x64 arm64; do cp "$E/keep-$prog-$arch" "$REL/$prog-$VERSION-linux-$arch"; done
done
BAD="0000000000000000000000000000000000000000000000000000000000000000"
check_not "--sha256 that disagrees with SHA256SUMS is refused" run_install --version "$VERSION" --sha256 "$BAD"
cp "$REL/SHA256SUMS" "$E/sums.keep"
grep -v " cstan-dash-" "$E/sums.keep" >"$REL/SHA256SUMS"
check_not "a SHA256SUMS without cstan-dash is refused" run_install --version "$VERSION"
grep -v " cstan-$VERSION" "$E/sums.keep" >"$REL/SHA256SUMS"
check_not "a SHA256SUMS without cstan is refused" run_install --version "$VERSION"
rm -f "$REL/SHA256SUMS"
check_not "a release without SHA256SUMS is refused" run_install --version "$VERSION"
check "the install is still the good one" test "$(cstan --version 2>/dev/null)" = "$GOOD"
new_env mismatch
make_release "$E/releases" 0.4.1 0.4.0
export CAPSTAN_RELEASE_BASE="file://$E/releases"
check_not "a binary that reports another version than the pin is refused" run_install --version 0.4.1
check_not "nothing installed for a version mismatch" test -e "$CAPSTAN_HOME/current"

# --- 6. the cutoff ---------------------------------------------------------------------
new_env cutoff
make_release "$E/releases" "$VERSION"
make_release "$E/releases" "$OLD_VERSION"
make_release "$E/releases" 0.3.9
export CAPSTAN_RELEASE_BASE="file://$E/releases"
for old in 0.3.0 0.2.0 0.1.1 0.3.9; do
  if run_install --version "$old" >"$E/out" 2>&1; then
    fail "--version $old is refused"
  else
    pass "--version $old is refused"
  fi
  check "the $old refusal names that tag's install.sh URL" contains "$E/out" "$RAW/v$old/install.sh"
  check_not "nothing installed for --version $old" test -e "$CAPSTAN_HOME/current"
  check_not "no download for --version $old" contains "$E/out" "Downloading"
done
if CAPSTAN_VERSION=0.3.0 run_install >"$E/out" 2>&1; then
  fail "CAPSTAN_VERSION=0.3.0 is refused"
else
  pass "CAPSTAN_VERSION=0.3.0 is refused"
fi
check "the CAPSTAN_VERSION refusal names the tag URL" contains "$E/out" "$RAW/v0.3.0/install.sh"
stub_latest 0.3.0
if PATH="$E/stub:$PATH" run_install >"$E/out" 2>&1; then fail "latest 0.3.0 installs nothing"; else pass "latest 0.3.0 installs nothing"; fi
check "latest 0.3.0 says the Rust release is not out yet" contains "$E/out" "is not out yet"
check "latest 0.3.0 names the v0.3.0 install.sh URL" contains "$E/out" "$RAW/v0.3.0/install.sh"
check_not "nothing installed when latest is 0.3.0" test -e "$CAPSTAN_HOME/current"
stub_latest 0.4.0
if PATH="$E/stub:$PATH" run_install >"$E/out" 2>&1; then pass "latest 0.4.0 installs"; else fail "latest 0.4.0 installs"; cat "$E/out"; fi
check "latest 0.4.0 installed cstan 0.4.0" test "$(cstan --version 2>/dev/null)" = "cstan 0.4.0"
new_env boundary
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
check "--version 0.4.0 installs" run_install --version 0.4.0
new_env bad-latest
export CAPSTAN_RELEASE_BASE="file://$E/releases"
stub_latest 1.2.3-rc1
if PATH="$E/stub:$PATH" run_install >"$E/out" 2>&1; then
  fail "a pre-release latest tag is refused"
else
  pass "a pre-release latest tag is refused"
fi
check "the refusal says it could not read a release version" contains "$E/out" "could not read a release version"
for bad in "0.4.0 [following]" "1.2" "1.2.3.4" "v1.2.3" "1.2.3-rc1" "1..3" ".1.2" "a.b.c" "1.2.3 "; do
  check_not "--version '$bad' is rejected" run_install --version "$bad"
done
CAPSTAN_VERSION="1.2.3x"
export CAPSTAN_VERSION
check_not "CAPSTAN_VERSION=1.2.3x is rejected" run_install
unset CAPSTAN_VERSION

# --- 7. the wget code path ---------------------------------------------------------------
new_env wget
make_release "$E/releases" "$VERSION"
cat >"$E/stub/wget" <<'STUB'
#!/bin/sh
# stub wget: a canned redirect for -S, file copies from $STUB_ROOT otherwise
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
cp "$E/releases/download/v$VERSION/"* "$E/stubroot/"
export CAPSTAN_FETCHER=wget STUB_ROOT="$E/stubroot" CAPSTAN_RELEASE_BASE="http://stub.invalid/releases"
STUB_LOCATION="https://github.com/akhilrs/capstan/releases/tag/v$VERSION"
export STUB_LOCATION
if PATH="$E/stub:$PATH" run_install >"$E/out" 2>&1; then
  pass "wget -S output with '[following]' resolves the latest version and installs"
else
  fail "wget -S output with '[following]' resolves the latest version and installs"
  cat "$E/out"
fi
check "wget-resolved install runs" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
check "no stray text in the resolved version" contains "$E/out" "Installed cstan $VERSION\$"
unset CAPSTAN_FETCHER STUB_ROOT STUB_LOCATION

# --- 8. upgrades from the earlier layouts ----------------------------------------------------
# sim_daemon <dir>: a long-running process whose executable lives in <dir>, standing in for a project's controller daemon.
sim_daemon() {
  mkdir -p "$1"
  cp "$(command -v sleep)" "$1/daemon-sim"
  "$1/daemon-sim" 120 &
  SIM_PIDS="$SIM_PIDS $!"
}

# fabricate_old <layout>: what the v0.2.0 / v0.3.0 install.sh left in $CAPSTAN_HOME/current, and the bin symlink.
fabricate_old() {
  old="$CAPSTAN_HOME/current"
  mkdir -p "$old/bin" "$CAPSTAN_BIN_DIR"
  case "$1" in
    sea)
      # v0.2.0 / v0.3.0 binary install: bin/cstan is the Node SEA binary; v0.3.0 may add the Rust dashboard.
      printf '#!/bin/sh\necho "cstan %s"\n' "$OLD_VERSION" >"$old/bin/cstan"
      printf '#!/bin/sh\necho "cstan-dash %s"\n' "$OLD_VERSION" >"$old/bin/cstan-dash"
      ;;
    front)
      # v0.3.0 with a front end: bin/cstan is the Rust front end, bin/cstan-node the SEA binary.
      printf '#!/bin/sh\ncase "${1:-}" in __front-version) echo "cstan-front %s" ;; *) echo "cstan %s" ;; esac\n' "$OLD_VERSION" "$OLD_VERSION" >"$old/bin/cstan"
      printf '#!/bin/sh\necho "cstan %s"\n' "$OLD_VERSION" >"$old/bin/cstan-node"
      printf '#!/bin/sh\necho "cstan-dash %s"\n' "$OLD_VERSION" >"$old/bin/cstan-dash"
      ;;
    npm)
      # the tarball install: npm install --global --prefix <staging>: bin/cstan links into lib/node_modules/capstan-controller.
      pkg="$old/lib/node_modules/capstan-controller"
      mkdir -p "$pkg/dist/src" "$pkg/node_modules/smol-toml"
      printf '{"name":"capstan-controller","version":"%s"}\n' "$OLD_VERSION" >"$pkg/package.json"
      printf '#!/bin/sh\necho "cstan %s"\n' "$OLD_VERSION" >"$pkg/dist/src/cli.js"
      chmod 755 "$pkg/dist/src/cli.js"
      printf 'module.exports = {};\n' >"$pkg/node_modules/smol-toml/index.js"
      ln -s ../lib/node_modules/capstan-controller/dist/src/cli.js "$old/bin/cstan"
      ;;
  esac
  chmod 755 "$old"/bin/* 2>/dev/null || true
  ln -sfn "$old/bin/cstan" "$CAPSTAN_BIN_DIR/cstan"
}

for layout in sea front npm; do
  new_env "upgrade-$layout"
  make_release "$E/releases" "$VERSION"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  fabricate_old "$layout"
  check "the fabricated $layout install reports $OLD_VERSION" test "$(cstan --version 2>/dev/null)" = "cstan $OLD_VERSION"
  case "$layout" in
    npm) sim_daemon "$CAPSTAN_HOME/current/lib/node_modules/capstan-controller/bin" ;;
    *) sim_daemon "$CAPSTAN_HOME/current/bin" ;;
  esac
  if run_install --version "$VERSION" >"$E/out" 2>&1; then pass "upgrade from $layout exits 0"; else fail "upgrade from $layout exits 0"; cat "$E/out"; fi
  check "upgrade from $layout installs cstan $VERSION" test "$(cstan --version 2>/dev/null)" = "cstan $VERSION"
  check "upgrade from $layout installs cstan-dash $VERSION" test "$("$CAPSTAN_HOME/current/bin/cstan-dash" --version 2>/dev/null)" = "cstan-dash $VERSION"
  check "upgrade from $layout leaves bin/cstan and bin/cstan-dash only" test "$(installed_files)" = "./bin ./bin/cstan ./bin/cstan-dash "
  check_not "upgrade from $layout removes cstan-node" test -e "$CAPSTAN_HOME/current/bin/cstan-node"
  check_not "upgrade from $layout removes the old npm files" test -e "$CAPSTAN_HOME/current/lib"
  check_not "upgrade from $layout leaves no current.old" test -e "$CAPSTAN_HOME/current.old"
  check "upgrade from $layout names the earlier version" contains "$E/out" "Upgrading Capstan $OLD_VERSION -> $VERSION"
  check "upgrade from $layout warns that a daemon of the old install runs" contains "$E/out" "still running"
  check "upgrade from $layout says to run cstan stop and cstan start" contains "$E/out" "cstan stop && cstan start"
  check "the $layout daemon was not killed by the installer" sh -c 'for pid in $1; do kill -0 "$pid" 2>/dev/null && exit 0; done; exit 1' _ "$SIM_PIDS"
  for pid in $SIM_PIDS; do kill "$pid" 2>/dev/null || true; done
  for pid in $SIM_PIDS; do wait "$pid" 2>/dev/null || true; done
  SIM_PIDS=""
  # the same upgrade without a running daemon: no warning
  new_env "upgrade-$layout-idle"
  make_release "$E/releases" "$VERSION"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  fabricate_old "$layout"
  run_install --version "$VERSION" >"$E/out" 2>&1 || fail "idle upgrade from $layout exits 0"
  check_not "no daemon warning for an idle $layout install" contains "$E/out" "still running"
  mkdir -p "$E/repo/.capstan"
  check "upgrade from $layout leaves project .capstan/ alone" test -d "$E/repo/.capstan"
done

# --- 9. --binary and --dash-binary (local files) -----------------------------------------------
new_env local
make_release "$E/releases" "$VERSION"
LOCAL="$E/releases/download/v$VERSION"
if [ -n "$HOST_ARCH" ]; then
  check "--binary with the SHA256SUMS beside it installs" run_install --binary "$LOCAL/cstan-$VERSION-linux-$HOST_ARCH"
  check "a local --binary installs cstan alone" test "$(installed_files)" = "./bin ./bin/cstan "
  check "--dash-binary adds cstan-dash" run_install --binary "$LOCAL/cstan-$VERSION-linux-$HOST_ARCH" --dash-binary "$LOCAL/cstan-dash-$VERSION-linux-$HOST_ARCH"
  check "the install then holds both" test "$(installed_files)" = "./bin ./bin/cstan ./bin/cstan-dash "
  check_not "--binary with a wrong --sha256 is refused" run_install --binary "$LOCAL/cstan-$VERSION-linux-$HOST_ARCH" --sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  check_not "--dash-binary and --no-dash are exclusive" run_install --binary "$LOCAL/cstan-$VERSION-linux-$HOST_ARCH" --dash-binary "$LOCAL/cstan-dash-$VERSION-linux-$HOST_ARCH" --no-dash
  check "--no-dash installs cstan alone from a release" sh -c 'export CAPSTAN_RELEASE_BASE="file://$1/releases"; sh "$2" --version "$3" --no-dash </dev/null' _ "$E" "$INSTALLER" "$VERSION"
  check "--no-dash left no cstan-dash" test "$(installed_files)" = "./bin ./bin/cstan "
fi
check_not "the removed --tarball option is unknown" run_install --tarball /nonexistent
check_not "the removed --no-binary option is unknown" run_install --no-binary
check_not "the removed --front-binary option is unknown" run_install --front-binary /nonexistent
check_not "a bin-dir entry that is not ours is refused" sh -c '
  mkdir -p "$CAPSTAN_BIN_DIR"; : >"$CAPSTAN_BIN_DIR/cstan"; sh "$1" --version "$2" </dev/null' _ "$INSTALLER" "$VERSION"

# --- 10. uninstall ---------------------------------------------------------------------------------
new_env uninstall
make_release "$E/releases" "$VERSION"
export CAPSTAN_RELEASE_BASE="file://$E/releases"
run_install --version "$VERSION" >/dev/null 2>&1 || fail "install for the uninstall scenario"
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

# --- 11. a real release, when one is given ----------------------------------------------------------
if [ -n "${CAPSTAN_TEST_RELEASE_DIR:-}" ] && [ -n "$HOST_ARCH" ]; then
  new_env real
  real_version="$(sed -n "s/^[0-9a-f]*  cstan-\\([0-9.]*\\)-linux-$HOST_ARCH\$/\\1/p" "$CAPSTAN_TEST_RELEASE_DIR/SHA256SUMS" | head -n 1)"
  mkdir -p "$E/releases/download/v$real_version"
  cp "$CAPSTAN_TEST_RELEASE_DIR"/* "$E/releases/download/v$real_version/"
  export CAPSTAN_RELEASE_BASE="file://$E/releases"
  check "the real release installs" run_install --version "$real_version"
  check "the real cstan reports $real_version" test "$(cstan --version 2>/dev/null)" = "cstan $real_version"
  check "the real cstan-dash reports $real_version" test "$("$CAPSTAN_HOME/current/bin/cstan-dash" --version 2>/dev/null)" = "cstan-dash $real_version"
else
  echo "skip the real release: CAPSTAN_TEST_RELEASE_DIR is not set"
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES install check(s) failed"
  exit 1
fi
echo "all install checks passed"
