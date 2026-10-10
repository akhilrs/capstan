#!/bin/sh
# Re-measures the section 2 baseline of docs/research/rust-port-estimate.md for the frozen Node build and the Rust cstan,
# side by side, on scratch projects only. By hand; it needs the Node build (npm run build) and is not part of scripts/check.sh.
#
#   sh docs/research/cutover-baseline/measure.sh <repository> <rust cstan binary> <work dir>
#
# <work dir> must be under /tmp/capstan-cutover- (it is created, filled and removed). The live .capstan ledger and other
# projects are never opened: every project is a fresh `cstan init` under <work dir>, or the generated ledger of
# scripts/shadow-daemon.mjs --generate. Per implementation it prints:
#   idle    scratch daemon: CPU of one core over 60 s (/proc utime + stime) and RSS (VmRSS)
#   call    ping, status --json and inbox: 20 runs each, wall time (ms, from the clock around /usr/bin/time) and max RSS (kB)
#   wait    RSS of a blocked `cstan wait` (VmRSS and VmHWM after 3 s) of an agent
#   large   status --json on a large generated ledger (50 agents, 5000 messages, 20 plans): 20 runs, then the daemon's RSS
set -eu

REPO="$1"
RUST="$2"
WORK="$3"
case "$WORK" in /tmp/capstan-cutover-*) ;; *) echo "measure: work dir must be under /tmp/capstan-cutover-" >&2; exit 2 ;; esac
NODE="$(command -v node)"
RUNS=20
IDLE_SECONDS=60
mkdir -p "$WORK/home"

# client <impl> <args...>: the Node CLI or the Rust cstan, in a minimal environment (no herdr, no agent identity).
client() {
  impl="$1"
  shift
  if [ "$impl" = node ]; then
    env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "$NODE" "$REPO/dist/src/cli.js" "$@"
  else
    env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "$RUST" "$@"
  fi
}

ticks() { sed 's/^.*) //' "/proc/$1/stat" | awk '{ print $12 + $13 }'; }
status_kb() { awk -v key="$2:" '$1 == key { print $2 }' "/proc/$1/status"; }

# stats <file>: "median min max" of the numbers in the file.
stats() { sort -n "$1" | awk '{ v[NR] = $1 } END { printf "%s %s %s", v[int((NR + 1) / 2)], v[1], v[NR] }'; }

# timed <impl> <label> <env...> -- <args>: RUNS runs, wall in ms and max RSS in kB.
timed() {
  impl="$1"; label="$2"; shift 2
  walls="$WORK/walls.$impl.$label"; rss="$WORK/rss.$impl.$label"; : >"$walls"; : >"$rss"
  i=0
  while [ "$i" -lt "$RUNS" ]; do
    start="$(date +%s%N)"
    if [ "$impl" = node ]; then
      env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "$@" /usr/bin/time -f '%M' -o "$WORK/time.out" "$NODE" "$REPO/dist/src/cli.js" $ARGS >/dev/null
    else
      env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "$@" /usr/bin/time -f '%M' -o "$WORK/time.out" "$RUST" $ARGS >/dev/null
    fi
    end="$(date +%s%N)"
    echo $(((end - start) / 1000000)) >>"$walls"
    cat "$WORK/time.out" >>"$rss"
    i=$((i + 1))
  done
  set -- $(stats "$walls") $(stats "$rss")
  echo "call $impl $label wall_ms median=$1 min=$2 max=$3 maxrss_kb median=$4 min=$5 max=$6"
}

new_project() {
  impl="$1"; dir="$2"
  mkdir -p "$dir"
  git -C "$dir" init --quiet
  git -C "$dir" -c user.name=t -c user.email=t@example.com commit --quiet --allow-empty -m "chore: initial commit"
  (cd "$dir" && client "$impl" init >/dev/null)
}

measure() {
  impl="$1"
  project="$WORK/$impl/p"
  new_project "$impl" "$project"
  tokens="$(node "$REPO/docs/research/cutover-baseline/seed.mjs" "$REPO" "$project")"
  pm_token="$(printf '%s' "$tokens" | sed 's/.*"pm":{[^}]*"token":"\([^"]*\)".*/\1/')"
  dev_token="$(printf '%s' "$tokens" | sed 's/.*"developer":{[^}]*"token":"\([^"]*\)".*/\1/')"
  cd "$project"
  client "$impl" start >/dev/null
  pid="$(cat .capstan/state/daemon.pid)"
  sleep 5
  before="$(ticks "$pid")"
  sleep "$IDLE_SECONDS"
  after="$(ticks "$pid")"
  cpu="$(awk -v a="$before" -v b="$after" -v s="$IDLE_SECONDS" 'BEGIN { printf "%.2f", (b - a) / s }')"
  echo "idle $impl daemon pid=$pid cpu_percent=$cpu rss_kb=$(status_kb "$pid" VmRSS) hwm_kb=$(status_kb "$pid" VmHWM) threads=$(awk '$1 == "Threads:" { print $2 }' "/proc/$pid/status")"
  socket="$project/.capstan/state/control.sock"
  ARGS="ping" timed "$impl" ping
  ARGS="status --json" timed "$impl" status
  ARGS="inbox" timed "$impl" inbox CAPSTAN_TOKEN="$pm_token" CAPSTAN_SOCKET="$socket" CAPSTAN_AGENT_ID=pm-1
  # A blocked wait: the developer waits with nothing queued; sample it while it is blocked, then end it.
  env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off CAPSTAN_TOKEN="$dev_token" CAPSTAN_SOCKET="$socket" CAPSTAN_AGENT_ID=developer-1 \
    $([ "$impl" = node ] && echo "$NODE $REPO/dist/src/cli.js" || echo "$RUST") wait >/dev/null 2>&1 &
  waiter=$!
  sleep 3
  if tr '\0' ' ' <"/proc/$waiter/cmdline" | grep -q ' wait \?$'; then
    echo "wait $impl blocked pid=$waiter rss_kb=$(status_kb "$waiter" VmRSS) hwm_kb=$(status_kb "$waiter" VmHWM)"
    kill "$waiter"
  else
    echo "wait $impl NOT-BLOCKED (the waiter ended early)"
  fi
  wait "$waiter" 2>/dev/null || true
  client "$impl" stop >/dev/null
  cd "$WORK"
}

large() {
  impl="$1"
  project="$WORK/large-$impl"
  # One generated project per implementation: a project's stateDirectory must stay where it was generated.
  "$NODE" "$REPO/scripts/shadow-daemon.mjs" --generate "$project" >/dev/null
  git -C "$project" init --quiet
  git -C "$project" -c user.name=t -c user.email=t@example.com commit --quiet --allow-empty -m "chore: initial commit"
  cd "$project"
  client "$impl" start >/dev/null
  pid="$(cat .capstan/state/daemon.pid)"
  ARGS="status --json" timed "$impl" large-status
  echo "large $impl daemon pid=$pid rss_kb=$(status_kb "$pid" VmRSS) hwm_kb=$(status_kb "$pid" VmHWM) ledger_bytes=$(wc -c <.capstan/state/controller.sqlite)"
  client "$impl" stop >/dev/null
  cd "$WORK"
}

echo "host $(uname -srm) cpus=$(nproc) node=$("$NODE" --version) rust_cstan=$("$RUST" --version)"
echo "ledger generate: scripts/shadow-daemon.mjs --generate (default 50 agents, 5000 messages, 20 plans), one per implementation"
# CSTAN_BASELINE_ONLY=large skips the small-project measurements.
if [ "${CSTAN_BASELINE_ONLY:-}" != large ]; then for impl in node rust; do measure "$impl"; done; fi
for impl in node rust; do large "$impl"; done
rm -rf "$WORK"
