#!/bin/sh
# Smoke test for the standalone cstan binaries (npm run build:binary first). With the Rust dashboard built (npm run
# build:dash, or CSTAN_DASH_SMOKE_BIN=<path to cstan-dash>) it also runs `cstan dash` in a pty both with cstan-dash
# beside the binary (the frame must come from cstan-dash) and without it (the Node dashboard and its hint).
# Runs each binary in a mkdtemp HOME and a temp git repository with a PATH that holds only the
# binary's directory plus /usr/bin:/bin and no node. Skips a target whose binary is missing or
# cannot run on this machine (arm64 needs qemu-aarch64 binfmt). Exit code 0 means every check passed.
# Usage: scripts/smoke-binary.sh [binary...]   (default: release/cstan-<version>-linux-{x64,arm64})
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$ROOT/package.json")"
FAILURES=0
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-smoke.XXXXXX")"
# A node for the npm-build ledger checks only; the binary never sees it.
NODE_BIN="$(command -v node || true)"
DAEMON_PIDS=""

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}

cleanup() {
  for pid in $DAEMON_PIDS; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$SANDBOX"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# check <description> <command...>: passes when the command succeeds.
check() {
  desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}

if [ "$#" -gt 0 ]; then
  BINARIES="$*"
else
  BINARIES="$ROOT/release/cstan-$VERSION-linux-x64 $ROOT/release/cstan-$VERSION-linux-arm64"
fi

# can_run <binary>: true when this machine can execute the binary's architecture.
can_run() {
  case "$(file -b "$1")" in
    *x86-64*) [ "$(uname -m)" = x86_64 ] ;;
    *aarch64* | *ARM\ aarch64*)
      [ "$(uname -m)" = aarch64 ] || [ -e /proc/sys/fs/binfmt_misc/qemu-aarch64 ]
      ;;
    *) return 1 ;;
  esac
}

# wait_gone <pid>: true once the process is gone (10 s at most).
wait_gone() {
  i=0
  while [ "$i" -lt 100 ]; do
    kill -0 "$1" 2>/dev/null || return 0
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

# dash_binary: the cstan-dash to test with (CSTAN_DASH_SMOKE_BIN, the x64 release build, or the build:dash output);
# prints its path, or nothing when none runs here.
dash_binary() {
  for candidate in "${CSTAN_DASH_SMOKE_BIN:-}" "$ROOT/release/cstan-dash-$VERSION-linux-x64" \
    "${CARGO_TARGET_DIR:-$ROOT/dash/target}/release/cstan-dash"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    case "$("$candidate" --version 2>/dev/null)" in "cstan-dash "*) printf '%s\n' "$candidate" && return 0 ;; esac
  done
  return 0
}

# dash_session <dir with cstan> <name>: runs `cstan dash` in a pty (160x45) from that directory, reads which program
# the process has become after 3 s, sends q, and leaves the pty output in $DIR/dash.<name>.out, the program in
# $DIR/dash.<name>.exe and the pid in $DIR/dash.<name>.pid.
dash_session() {
  pidfile="$DIR/dash.$2.pid"
  exefile="$DIR/dash.$2.exe"
  out="$DIR/dash.$2.out"
  rm -f "$pidfile" "$exefile"
  (
    sleep 3
    readlink "/proc/$(cat "$pidfile" 2>/dev/null)/exe" >"$exefile" 2>/dev/null || true
    printf q
    sleep 3
  ) | timeout -s KILL 25 env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm \
    PATH="$1:/usr/bin:/bin" script -qec "stty rows 45 cols 160; echo \$\$ >'$pidfile'; exec cstan dash --no-color" /dev/null \
    >"$out" 2>&1 || true
}

smoke() {
  BINARY="$1"
  NAME="$(basename "$BINARY")"
  [ -f "$BINARY" ] || {
    echo "skip $NAME: not built"
    return 0
  }
  if ! can_run "$BINARY"; then
    echo "skip $NAME: cannot run this architecture here (arm64 needs qemu-aarch64 binfmt)"
    return 0
  fi
  echo "== $NAME"
  DIR="$(mktemp -d "$SANDBOX/run.XXXXXX")"
  mkdir -p "$DIR/home" "$DIR/bin" "$DIR/repo" "$DIR/tmp"
  ln -s "$(cd "$(dirname "$BINARY")" && pwd)/$NAME" "$DIR/bin/cstan"
  REAL="$(readlink -f "$BINARY")"
  # A scrubbed environment: no CAPSTAN_* variables from an enclosing agent, no node on PATH.
  c() {
    env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm \
      PATH="$DIR/bin:/usr/bin:/bin" "$@"
  }
  (cd "$DIR/repo" &&
    git init -q . &&
    git -c user.name=smoke -c user.email=smoke@example.invalid commit -q --allow-empty -m init)

  if c sh -c 'command -v node' >/dev/null 2>&1; then
    fail "$NAME: node is on the restricted PATH"
    return 0
  fi
  pass "$NAME: no node on PATH"

  GOT="$(cd "$DIR/repo" && c cstan --version 2>&1)" || true
  [ "$GOT" = "cstan $VERSION" ] && pass "$NAME: --version is $GOT" || fail "$NAME: --version printed '$GOT'"
  cd "$DIR/repo"
  # The bundle cache: written once, reused, and rewritten when tampered with.
  CACHED="$(ls "$DIR"/home/.cache/capstan/sea/*/cstan.mjs 2>/dev/null | head -n 1)"
  if [ -n "$CACHED" ]; then
    INODE="$(stat -c %i "$CACHED")"
    c cstan --version >/dev/null 2>&1 || true
    [ "$(stat -c %i "$CACHED")" = "$INODE" ] && pass "$NAME: second run reuses the bundle cache" || fail "$NAME: bundle cache was rewritten"
    [ "$(stat -c %a "$(dirname "$CACHED")")" = 700 ] && [ "$(stat -c %a "$CACHED")" = 600 ] && pass "$NAME: bundle cache is private" || fail "$NAME: bundle cache modes"
    echo "// tampered" >>"$CACHED"
    GOT="$(c cstan --version 2>&1)" || true
    [ "$GOT" = "cstan $VERSION" ] && ! grep -q tampered "$CACHED" && pass "$NAME: tampered cache file is rewritten" || fail "$NAME: tampered cache ('$GOT')"
  else
    fail "$NAME: no bundle cache under HOME/.cache/capstan/sea"
  fi
  check "$NAME: --help" c cstan --help
  check "$NAME: init" c cstan init
  # start brings the daemon up; the pane launch needs herdr, so its exit status is not checked.
  c cstan start >"$DIR/start.out" 2>&1 || true
  grep -q "running: true" "$DIR/start.out" && pass "$NAME: start" || fail "$NAME: start ($(head -c 300 "$DIR/start.out"))"
  PID="$(cat .capstan/state/daemon.pid 2>/dev/null || true)"
  DAEMON_PIDS="$DAEMON_PIDS $PID"
  if [ -n "$PID" ] && [ "$(readlink -f "/proc/$PID/exe" 2>/dev/null)" = "$REAL" ]; then
    pass "$NAME: daemon pid $PID runs the binary itself"
  else
    fail "$NAME: daemon pid '$PID' is not the binary"
  fi
  check "$NAME: status" c cstan status
  OUT="$(c cstan inbox --hook 2>&1)" && RC=0 || RC=$?
  [ "$RC" -eq 0 ] && [ -z "$OUT" ] && pass "$NAME: inbox --hook silent, exit 0" || fail "$NAME: inbox --hook rc=$RC output '$OUT'"

  # The wrapper the launcher writes for agents; without herdr no pane opens, so it may be absent.
  WRAPPER="$DIR/repo/.capstan/bin/cstan"
  if [ ! -x "$WRAPPER" ]; then
    mkdir -p "$DIR/repo/.capstan/bin"
    printf '#!/bin/sh\nexec %s "$@"\n' "$REAL" >"$WRAPPER"
    chmod 700 "$WRAPPER"
    echo "note $NAME: launcher wrote no wrapper (no herdr); using the same one-line shape"
  fi
  GOT="$(c "$WRAPPER" --version 2>&1)" || true
  [ "$GOT" = "cstan $VERSION" ] && pass "$NAME: wrapper --version" || fail "$NAME: wrapper --version '$GOT'"
  OUT="$(c "$WRAPPER" inbox --hook 2>&1)" && RC=0 || RC=$?
  [ "$RC" -eq 0 ] && [ -z "$OUT" ] && pass "$NAME: wrapper inbox --hook" || fail "$NAME: wrapper inbox --hook rc=$RC '$OUT'"

  # One dash frame in a pty.
  if command -v script >/dev/null 2>&1; then
    # SIGKILL: dash handles SIGTERM itself and would otherwise run until the pty closes.
    timeout -s KILL 15 env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm \
      PATH="$DIR/bin:/usr/bin:/bin" script -qec "timeout -s KILL 4 cstan dash --no-color" /dev/null \
      </dev/null >"$DIR/dash.out" 2>&1 || true
    if [ -s "$DIR/dash.out" ] && ! grep -qiE "cannot find|ERR_MODULE|wasm|ExperimentalWarning|Error:" "$DIR/dash.out"; then
      pass "$NAME: dash renders a frame"
    else
      fail "$NAME: dash ($(head -c 300 "$DIR/dash.out"))"
    fi
  else
    echo "skip $NAME: dash (no script(1))"
  fi

  # The Rust dashboard: cstan dash becomes cstan-dash when it sits beside the binary, else the Node dashboard runs.
  if command -v script >/dev/null 2>&1; then
    DASHBIN="$(dash_binary)"
    case "$(file -b "$REAL")" in *x86-64*) NATIVE=1 ;; *) NATIVE=0 ;; esac
    if [ -z "$DASHBIN" ] || [ "$NATIVE" = 0 ] || [ "$(uname -m)" != x86_64 ]; then
      echo "skip $NAME: rust dash (no runnable x86-64 cstan-dash; npm run build:dash)"
    else
      for variant in without with; do
        VDIR="$DIR/$variant"
        mkdir -p "$VDIR"
        # A copy of the binary (a symlink would resolve back to its own directory, where cstan-dash may sit).
        ln "$REAL" "$VDIR/cstan" 2>/dev/null || cp "$REAL" "$VDIR/cstan"
        [ "$variant" = with ] && cp "$DASHBIN" "$VDIR/cstan-dash"
        dash_session "$VDIR" "$variant"
        EXE="$(cat "$DIR/dash.$variant.exe" 2>/dev/null || true)"
        DPID="$(cat "$DIR/dash.$variant.pid" 2>/dev/null || true)"
        if [ "$variant" = with ]; then
          [ "$(basename "$EXE")" = cstan-dash ] && pass "$NAME: cstan dash is cstan-dash itself (/proc/$DPID/exe)" ||
            fail "$NAME: cstan dash with cstan-dash beside it runs '$EXE'"
          grep -aq "queue" "$DIR/dash.with.out" && grep -aq "agents" "$DIR/dash.with.out" &&
            pass "$NAME: cstan-dash draws a frame" || fail "$NAME: no cstan-dash frame ($(head -c 300 "$DIR/dash.with.out"))"
          if [ -n "$DPID" ] && wait_gone "$DPID"; then pass "$NAME: q quits cstan-dash"; else fail "$NAME: cstan-dash did not quit on q"; fi
          ! grep -aq "using the Node dashboard" "$DIR/dash.with.out" && pass "$NAME: no Node-dash hint beside cstan-dash" ||
            fail "$NAME: the Node-dash hint appeared although cstan-dash is beside the binary"
        else
          [ -n "$EXE" ] && [ "$(basename "$EXE")" != cstan-dash ] && pass "$NAME: without cstan-dash the Node dashboard runs ($EXE)" ||
            fail "$NAME: cstan dash without cstan-dash runs '$EXE'"
          grep -aq "queue" "$DIR/dash.without.out" && pass "$NAME: the Node dashboard draws a frame" ||
            fail "$NAME: no Node dashboard frame ($(head -c 300 "$DIR/dash.without.out"))"
          grep -aq "using the Node dashboard" "$DIR/dash.without.out" && pass "$NAME: the hint line to install cstan-dash appears" ||
            fail "$NAME: no Node-dash hint line"
        fi
        rm -f "$VDIR/cstan" "$VDIR/cstan-dash"
      done
    fi
  fi

  # Warnings on stderr (SEA or node:sqlite) would show here.
  ERR="$(c cstan status 2>&1 >/dev/null)" || true
  [ -z "$ERR" ] && pass "$NAME: nothing on stderr" || fail "$NAME: stderr '$ERR'"

  check "$NAME: stop" c cstan stop
  if [ -n "$PID" ] && wait_gone "$PID"; then pass "$NAME: daemon pid gone"; else fail "$NAME: daemon pid $PID still alive"; fi

  # Ledger compatibility with the npm build, both directions.
  if [ -n "$NODE_BIN" ] && [ -f "$ROOT/dist/src/cli.js" ]; then
    n() { env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" PATH="$(dirname "$NODE_BIN"):/usr/bin:/bin" "$@"; }
    # binary-made ledger opened by npm build
    n node "$ROOT/dist/src/cli.js" start >"$DIR/npm1.out" 2>&1 || true
    grep -q "running: true" "$DIR/npm1.out" && pass "$NAME: npm build opens the binary's ledger" || fail "$NAME: npm build on binary ledger ($(head -c 300 "$DIR/npm1.out"))"
    n node "$ROOT/dist/src/cli.js" stop >/dev/null 2>&1 || true
    # npm-made ledger opened by the binary
    mkdir -p "$DIR/repo2"
    (cd "$DIR/repo2" && git init -q . && git -c user.name=s -c user.email=s@example.invalid commit -q --allow-empty -m i &&
      n node "$ROOT/dist/src/cli.js" init >/dev/null 2>&1 &&
      { n node "$ROOT/dist/src/cli.js" start >/dev/null 2>&1 || true; } &&
      n node "$ROOT/dist/src/cli.js" stop >/dev/null 2>&1) || true
    (cd "$DIR/repo2" && c cstan start >"$DIR/bin1.out" 2>&1 || true)
    grep -q "running: true" "$DIR/bin1.out" && pass "$NAME: binary opens the npm build's ledger" || fail "$NAME: binary on npm ledger ($(head -c 300 "$DIR/bin1.out"))"
    (cd "$DIR/repo2" && c cstan stop >/dev/null 2>&1) || true
  else
    echo "skip $NAME: ledger compatibility (no node or dist/ build)"
  fi
  cd "$ROOT"
}

for b in $BINARIES; do smoke "$b"; done

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES smoke check(s) failed"
  exit 1
fi
echo "all smoke checks passed"
