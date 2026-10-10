#!/bin/sh
# Smoke test for the Rust binaries: cstan (which is also the controller daemon) and cstan-dash. Runs each cstan in a
# mkdtemp HOME and a temp git repository with a PATH that holds only that binary's directory plus /usr/bin:/bin and no
# node, and checks:
#   - cstan --version, --help, init
#   - `cstan daemon` started with CAPSTAN_LAUNCH=off, then status, ping, send/inbox/ack and stop
#   - a ledger made by Node (test/fixtures/ledger-better-sqlite3.sqlite, migration 31) migrates to the current
#     migration with the same schema_migrations rows and checksums, and the agents in it answer send/inbox
#   - `cstan dash` in a real terminal (a pty from script(1)) becomes a real cstan-dash beside it, draws its first frame
#     and ends cleanly on q
# and runs scripts/check-version.sh. A binary that cannot run on this machine is skipped (arm64 needs qemu-aarch64
# binfmt, or a native arm64 machine); the Release workflow runs the arm64 binaries on a native runner.
# Usage: scripts/smoke-binary.sh [cstan binary...]
#   default: release/cstan-<version>-linux-{x64,arm64}, else the host build under ${CARGO_TARGET_DIR}/release/cstan
#   CSTAN_DASH_SMOKE_BIN=<cstan-dash> names the dashboard; default: the one next to the release binary, else the host
#   build (${CARGO_TARGET_DIR}/release/cstan-dash or dash/target/release/cstan-dash)
# Exit code 0 means every check passed.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(tr -d ' \t\r\n' <"$ROOT/VERSION")"
FAILURES=0
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-smoke.XXXXXX")"
DAEMON_PIDS=""

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}

cleanup() {
  for pid in $DAEMON_PIDS; do
    # only a process this script started, and only while it still runs the binary under test
    if [ -d "/proc/$pid" ] && [ "$(readlink "/proc/$pid/cwd" 2>/dev/null | cut -c1-${#SANDBOX})" = "$SANDBOX" ]; then
      kill "$pid" 2>/dev/null || true
    fi
  done
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

# wait_for <file>: true once the file (a socket) exists (10 s at most).
wait_for() {
  i=0
  while [ "$i" -lt 100 ]; do
    [ -S "$1" ] || [ -f "$1" ] && return 0
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

# dash_binary <platform of the cstan under test>: the cstan-dash to test with; prints its path, or nothing when none runs here.
dash_binary() {
  for candidate in "${CSTAN_DASH_SMOKE_BIN:-}" "$ROOT/release/cstan-dash-$VERSION-$1" \
    "${CARGO_TARGET_DIR:+$CARGO_TARGET_DIR/release/cstan-dash}" "$ROOT/dash/target/release/cstan-dash"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    can_run "$candidate" || continue
    case "$(file -b "$candidate")" in
      *x86-64*) [ "$1" = linux-x64 ] || continue ;;
      *aarch64* | *ARM\ aarch64*) [ "$1" = linux-arm64 ] || continue ;;
    esac
    case "$("$candidate" --version 2>/dev/null)" in "cstan-dash $VERSION") printf '%s\n' "$candidate" && return 0 ;; esac
  done
  return 0
}

# dash_session <dir with cstan> <name>: runs `cstan dash` in a pty (160x45) from that directory, reads which program
# the process has become after 3 s, sends q, and leaves the pty output in $DIR/dash.<name>.out, the program in
# $DIR/dash.<name>.exe, the pid in $DIR/dash.<name>.pid and cstan's exit code in $DIR/dash.<name>.rc.
dash_session() {
  pidfile="$DIR/dash.$2.pid"
  exefile="$DIR/dash.$2.exe"
  rcfile="$DIR/dash.$2.rc"
  out="$DIR/dash.$2.out"
  rm -f "$pidfile" "$exefile" "$rcfile"
  (
    sleep 3
    readlink "/proc/$(cat "$pidfile" 2>/dev/null)/exe" >"$exefile" 2>/dev/null || true
    printf q
    sleep 3
  ) | {
    timeout -s KILL 25 env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm CAPSTAN_LAUNCH=off \
      PATH="$1:/usr/bin:/bin" script -qefc "stty rows 45 cols 160; echo \$\$ >'$pidfile'; exec cstan dash --no-color" /dev/null \
      >"$out" 2>&1
    echo $? >"$rcfile"
  } || true
}

smoke() {
  BINARY="$1"
  NAME="$(basename "$BINARY")"
  [ -f "$BINARY" ] || {
    echo "skip $NAME: not built"
    return 0
  }
  if ! can_run "$BINARY"; then
    echo "skip $NAME: cannot run this architecture here (arm64 needs qemu-aarch64 binfmt or an arm64 machine)"
    return 0
  fi
  case "$(file -b "$BINARY")" in *x86-64*) PLAT=linux-x64 ;; *) PLAT=linux-arm64 ;; esac
  echo "== $NAME"
  DIR="$(mktemp -d "$SANDBOX/run.XXXXXX")"
  mkdir -p "$DIR/home" "$DIR/bin" "$DIR/repo" "$DIR/tmp"
  cp "$BINARY" "$DIR/bin/cstan"
  chmod 755 "$DIR/bin/cstan"
  REAL="$DIR/bin/cstan"
  # A scrubbed environment: no CAPSTAN_* variables from an enclosing agent, no node on PATH.
  c() {
    env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm CAPSTAN_LAUNCH=off \
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

  cd "$DIR/repo"
  GOT="$(c cstan --version 2>&1)" || true
  [ "$GOT" = "cstan $VERSION" ] && pass "$NAME: --version is $GOT" || fail "$NAME: --version printed '$GOT'"
  check "$NAME: --help" c cstan --help
  check "$NAME: init" c cstan init

  # The daemon: `cstan daemon` in the project with launching off (no Herdr panes), left running in the background.
  start_daemon() {
    rm -f "$1/.capstan/state/control.sock"
    (cd "$1" && exec env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" LANG=C.UTF-8 TERM=xterm CAPSTAN_LAUNCH=off \
      PATH="$DIR/bin:/usr/bin:/bin" cstan daemon >"$DIR/daemon.out" 2>&1 </dev/null) &
    DPID=$!
    DAEMON_PIDS="$DAEMON_PIDS $DPID"
    wait_for "$1/.capstan/state/control.sock"
  }
  if start_daemon "$DIR/repo"; then pass "$NAME: cstan daemon serves its socket"; else fail "$NAME: cstan daemon did not serve ($(head -c 300 "$DIR/daemon.out"))"; fi
  PID="$DPID"
  if [ "$(readlink -f "/proc/$PID/exe" 2>/dev/null)" = "$(readlink -f "$REAL")" ]; then
    pass "$NAME: daemon pid $PID runs the binary itself"
  else
    fail "$NAME: daemon pid '$PID' is not the binary"
  fi
  check "$NAME: ping" c cstan ping
  check "$NAME: status" c cstan status
  ERR="$(c cstan status 2>&1 >/dev/null)" || true
  [ -z "$ERR" ] && pass "$NAME: nothing on stderr" || fail "$NAME: stderr '$ERR'"
  OUT="$(c cstan inbox --hook 2>&1)" && RC=0 || RC=$?
  # No agent token here: the hook says nothing and does not fail a tool call.
  [ "$RC" -eq 0 ] && [ -z "$OUT" ] && pass "$NAME: inbox --hook silent, exit 0" || fail "$NAME: inbox --hook rc=$RC output '$OUT'"
  check "$NAME: stop" c cstan stop
  if wait_gone "$PID"; then pass "$NAME: daemon pid gone"; else fail "$NAME: daemon pid $PID still alive"; fi
  wait "$PID" 2>/dev/null || true

  smoke_ledger
  smoke_dash
  cd "$ROOT"
}

# smoke_ledger: the Node-made fixture ledger (schema v31, with a PM and a developer agent) migrates to the current
# version with the same schema_migrations checksums, a pre-migration backup is written, and send/inbox/ack work on it.
smoke_ledger() {
  FIXTURE="$ROOT/test/fixtures/ledger-better-sqlite3.sqlite"
  if ! command -v python3 >/dev/null 2>&1; then
    echo "skip $NAME: ledger migration (python3 reads the sqlite files)"
    return 0
  fi
  LDIR="$DIR/ledger-repo"
  mkdir -p "$LDIR"
  (cd "$LDIR" &&
    git init -q . &&
    git -c user.name=smoke -c user.email=smoke@example.invalid commit -q --allow-empty -m init)
  check "$NAME: ledger: init" sh -c 'cd "$1" && shift && "$@"' _ "$LDIR" env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" PATH="$DIR/bin:/usr/bin:/bin" cstan init
  STATE="$LDIR/.capstan/state"
  rm -f "$STATE"/controller.sqlite*
  cp "$FIXTURE" "$STATE/controller.sqlite"
  # The fixture's own credentials are unknown (only their hashes are stored), so the copy gets known ones: this
  # project's operator key and two fixed tokens for the PM and the developer agent. schema_migrations is not touched.
  python3 -I - "$LDIR" <<'PY'
import hashlib, re, sqlite3, sys

root = sys.argv[1]
db = sqlite3.connect(root + "/.capstan/state/controller.sqlite")
project_id, name = db.execute("select project_id, name from projects").fetchone()
key = open(root + "/.capstan/operator.key").read().strip()
for role, token in (
    ("operator", key),
    ("PM", "smoke-pm-token-0123456789abcdefghijklmnop"),
    ("Developer", "smoke-dev-token-0123456789abcdefghijklmnop"),
):
    db.execute("update actors set credential_hash = ? where role = ?", (hashlib.sha256(token.encode()).hexdigest(), role))
db.commit()
config = root + "/.capstan/project.json"
text = open(config).read()
text = re.sub(r'"projectId": "[^"]*"', '"projectId": "%s"' % project_id, text)
text = re.sub(r'"name": "[^"]*"', '"name": "%s"' % name, text)
open(config, "w").write(text)  # in place: keeps the 0600 mode
PY
  if start_daemon "$LDIR"; then pass "$NAME: ledger: the daemon opens the Node-made ledger"; else fail "$NAME: ledger: the daemon did not serve ($(head -c 400 "$DIR/daemon.out"))"; fi
  LPID="$DPID"
  cl() { (cd "$LDIR" && c "$@"); }
  check "$NAME: ledger: ping" cl cstan ping
  smoke_messages
  check "$NAME: ledger: stop" cl cstan stop
  if wait_gone "$LPID"; then pass "$NAME: ledger: daemon pid gone"; else fail "$NAME: ledger: daemon pid $LPID still alive"; fi
  wait "$LPID" 2>/dev/null || true
  RESULT="$(python3 -I - "$FIXTURE" "$STATE/controller.sqlite" "$ROOT/migrations" <<'PY'
import glob, hashlib, os, sqlite3, sys

fixture, migrated, migrations = sys.argv[1:4]
old = sqlite3.connect("file:" + fixture + "?mode=ro", uri=True)
new = sqlite3.connect("file:" + migrated + "?mode=ro", uri=True)
before = old.execute("select version, name, checksum from schema_migrations order by version").fetchall()
after = new.execute("select version, name, checksum from schema_migrations order by version").fetchall()
files = sorted(os.path.basename(p) for p in glob.glob(os.path.join(migrations, "*.sql")))
problems = []
if len(after) != len(files):
    problems.append("%d rows for %d migration files" % (len(after), len(files)))
if after[: len(before)] != before:
    problems.append("the rows of the Node-made ledger changed")
for version, name, checksum in after:
    digest = hashlib.sha256(open(os.path.join(migrations, name), "rb").read()).hexdigest()
    if digest != checksum:
        problems.append("%s: checksum is not the sha256 of the file" % name)
print("; ".join(problems) if problems else "ok %d->%d" % (len(before), len(after)))
PY
  )" || RESULT="python failed"
  case "$RESULT" in
    ok*) pass "$NAME: ledger: schema_migrations migrated ($RESULT), same checksums" ;;
    *) fail "$NAME: ledger: $RESULT" ;;
  esac
  [ -n "$(ls "$STATE"/controller.sqlite.pre-v*.sqlite 2>/dev/null)" ] && pass "$NAME: ledger: pre-migration backup written" || fail "$NAME: ledger: no pre-migration backup"
}

# smoke_messages: send, inbox and ack as the agents of the fixture (the developer sends to the PM, the PM reads and
# acknowledges), over the project's socket with the tokens the copy was given.
smoke_messages() {
  SOCK="$LDIR/.capstan/state/control.sock"
  PM_TOKEN=smoke-pm-token-0123456789abcdefghijklmnop
  DEV_TOKEN=smoke-dev-token-0123456789abcdefghijklmnop
  agent() {
    token="$1"
    shift
    (cd "$LDIR" && c env CAPSTAN_TOKEN="$token" CAPSTAN_SOCKET="$SOCK" "$@")
  }
  SENT="$(agent "$DEV_TOKEN" cstan send @pm "smoke hello" 2>&1)" && SRC=0 || SRC=$?
  MID="$(printf '%s\n' "$SENT" | sed -n 's/^messageId: //p' | head -n 1)"
  [ "$SRC" -eq 0 ] && [ -n "$MID" ] && pass "$NAME: ledger: developer send @pm (message $MID)" || fail "$NAME: ledger: send rc=$SRC '$SENT'"
  agent "$PM_TOKEN" cstan inbox >"$DIR/inbox.out" 2>&1 && grep -q "$MID" "$DIR/inbox.out" && grep -q "smoke hello" "$DIR/inbox.out" &&
    pass "$NAME: ledger: PM inbox holds the message" || fail "$NAME: ledger: PM inbox ($(head -c 300 "$DIR/inbox.out"))"
  agent "$PM_TOKEN" cstan ack "$MID" >"$DIR/ack.out" 2>&1 && grep -q "acked" "$DIR/ack.out" &&
    pass "$NAME: ledger: PM ack" || fail "$NAME: ledger: ack ($(head -c 300 "$DIR/ack.out"))"
  agent "$PM_TOKEN" cstan inbox >"$DIR/inbox2.out" 2>&1 && ! grep -q "$MID" "$DIR/inbox2.out" &&
    pass "$NAME: ledger: the acknowledged message is gone from the inbox" || fail "$NAME: ledger: inbox after ack ($(head -c 300 "$DIR/inbox2.out"))"
  check "$NAME: ledger: operator inbox developer-agent" cl cstan inbox developer-agent
}

# smoke_dash: `cstan dash` in a pty beside a real cstan-dash.
smoke_dash() {
  if ! command -v script >/dev/null 2>&1; then
    echo "skip $NAME: dash (no script(1))"
    return 0
  fi
  DASHBIN="$(dash_binary "$PLAT")"
  if [ -z "$DASHBIN" ]; then
    echo "skip $NAME: dash (no runnable cstan-dash $VERSION for $PLAT; build it or set CSTAN_DASH_SMOKE_BIN)"
    return 0
  fi
  VDIR="$DIR/withdash"
  mkdir -p "$VDIR"
  cp "$REAL" "$VDIR/cstan"
  cp "$DASHBIN" "$VDIR/cstan-dash"
  chmod 755 "$VDIR/cstan" "$VDIR/cstan-dash"
  mkdir -p "$DIR/dashrepo"
  (cd "$DIR/dashrepo" &&
    git init -q . &&
    git -c user.name=smoke -c user.email=smoke@example.invalid commit -q --allow-empty -m init &&
    env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" PATH="$VDIR:/usr/bin:/bin" "$VDIR/cstan" init >/dev/null 2>&1)
  cd "$DIR/dashrepo"
  dash_session "$VDIR" real
  EXE="$(cat "$DIR/dash.real.exe" 2>/dev/null || true)"
  DPID2="$(cat "$DIR/dash.real.pid" 2>/dev/null || true)"
  [ "$(basename "$EXE")" = cstan-dash ] && pass "$NAME: dash: cstan dash is cstan-dash itself (/proc/$DPID2/exe)" ||
    fail "$NAME: dash: cstan dash runs '$EXE'"
  grep -aq "queue" "$DIR/dash.real.out" && grep -aq "agents" "$DIR/dash.real.out" &&
    pass "$NAME: dash: cstan-dash drew its first frame" || fail "$NAME: dash: no cstan-dash frame ($(head -c 300 "$DIR/dash.real.out"))"
  if [ -n "$DPID2" ] && wait_gone "$DPID2"; then pass "$NAME: dash: q quits cstan-dash"; else fail "$NAME: dash: cstan-dash did not quit on q"; fi
  [ "$(cat "$DIR/dash.real.rc" 2>/dev/null)" = 0 ] && pass "$NAME: dash: it ended cleanly (exit 0)" ||
    fail "$NAME: dash: exit code '$(cat "$DIR/dash.real.rc" 2>/dev/null)'"
  # `cstan dash` started a daemon for the project; stop it.
  DASHDAEMON="$(cat "$DIR/dashrepo/.capstan/state/daemon.pid" 2>/dev/null || true)"
  (cd "$DIR/dashrepo" && env -i HOME="$DIR/home" TMPDIR="$DIR/tmp" PATH="$VDIR:/usr/bin:/bin" "$VDIR/cstan" stop >/dev/null 2>&1) || true
  if [ -n "$DASHDAEMON" ]; then wait_gone "$DASHDAEMON" || fail "$NAME: dash: its daemon $DASHDAEMON is still alive"; fi
}

if sh "$ROOT/scripts/check-version.sh" "$ROOT" >"$SANDBOX/check-version.out" 2>&1; then
  pass "check-version.sh: $(cat "$SANDBOX/check-version.out")"
else
  fail "check-version.sh: $(cat "$SANDBOX/check-version.out")"
fi

if [ "$#" -gt 0 ]; then
  BINARIES="$*"
else
  BINARIES=""
  for plat in linux-x64 linux-arm64; do
    [ -f "$ROOT/release/cstan-$VERSION-$plat" ] && BINARIES="$BINARIES $ROOT/release/cstan-$VERSION-$plat"
  done
  if [ -z "$BINARIES" ] && [ -n "${CARGO_TARGET_DIR:-}" ] && [ -x "$CARGO_TARGET_DIR/release/cstan" ]; then
    BINARIES="$CARGO_TARGET_DIR/release/cstan"
  fi
  [ -n "$BINARIES" ] || echo "skip: no binaries (release/cstan-$VERSION-linux-*, or a release build under CARGO_TARGET_DIR)"
fi

for b in $BINARIES; do smoke "$b"; done

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES smoke check(s) failed"
  exit 1
fi
echo "all smoke checks passed"
