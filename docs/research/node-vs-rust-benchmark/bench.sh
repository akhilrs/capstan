#!/bin/bash
# Node v0.3.0 vs Rust v0.4.0, interleaved, on scratch projects only. By hand; see docs/research/node-vs-rust-benchmark.md.
#
#   bash bench.sh <work dir> <rounds> <sides...>
#
# <work dir> must be under /tmp/capstan-cutover- and hold (see "How to rerun" in the report):
#   dl3/cstan-0.3.0-linux-x64 cstan-dash-0.3.0-linux-x64   the published v0.3.0 Node standalone and its dashboard
#   dl4/cstan-0.4.0-linux-x64 cstan-dash-0.4.0-linux-x64   the published v0.4.0 Rust binary and its dashboard
#   npm3/package                                           capstan-controller-0.3.0.tgz unpacked, `npm ci --omit=dev` run in it
#   genroot/{scripts/shadow-daemon.mjs,scripts/copy-ledger.mjs,dist -> npm3/package/dist}   the large-ledger generator
# Sides: node (cstan-0.3.0 standalone), rust (cstan-0.4.0), npm (node + npm3/package/dist/src/cli.js, optional).
# Round r runs every side once, in order (A/B/A/B). Output: $WORK/results.tsv, lines "round<TAB>side<TAB>metric<TAB>value".
# Every project is a fresh `cstan init` under <work dir>; no other ledger is opened. Only daemons started here are signalled.
set -u
WORK="$1"; ROUNDS="$2"; shift 2; SIDES=("$@")
case "$WORK" in /tmp/capstan-cutover-*) ;; *) echo "bench: work dir must be under /tmp/capstan-cutover-" >&2; exit 2 ;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$WORK/results.tsv"; touch "$OUT"
NODE="$(command -v node)"
RUNS="${RUNS:-20}"; IDLE_SECONDS="${IDLE_SECONDS:-60}"; DASH_SECONDS="${DASH_SECONDS:-30}"
mkdir -p "$WORK/home"

setup_side() {
  case "$1" in
    node) CMD=("$WORK/dl3/cstan-0.3.0-linux-x64"); DASHBIN="$WORK/dl3/cstan-dash-0.3.0-linux-x64" ;;
    rust) CMD=("$WORK/dl4/cstan-0.4.0-linux-x64"); DASHBIN="$WORK/dl4/cstan-dash-0.4.0-linux-x64" ;;
    npm)  CMD=("$NODE" "$WORK/npm3/package/dist/src/cli.js"); DASHBIN="" ;;
  esac
}
emit() { printf '%s\t%s\t%s\t%s\n' "$ROUND" "$SIDE" "$1" "$2" >>"$OUT"; }
now() { date +%s%N; }
ms() { echo $((($2 - $1) / 1000000)); }
# cs <args>: one cstan call in the minimal environment; XE holds identity variables.
XE=()
cs() { env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" "${CMD[@]}" "$@"; }
ticks() { sed 's/^.*) //' "/proc/$1/stat" | awk '{ print $12 + $13 }'; }
kb() { awk -v key="$2:" '$1 == key { print $2 }' "/proc/$1/status" 2>/dev/null; }
# own <pid> <project>: true when the pid's cwd is the scratch project (daemons are started with the project as cwd).
own() { [ "$(readlink "/proc/$1/cwd" 2>/dev/null)" = "$2" ]; }

# timed <metric> <args...>: RUNS runs; wall (ms) and max RSS (kB) per run go to the results.
timed() {
  metric="$1"; shift
  i=0
  while [ "$i" -lt "$RUNS" ]; do
    s=$(now)
    env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" /usr/bin/time -f '%M' -o "$WORK/time.out" "${CMD[@]}" "$@" >/dev/null 2>&1
    e=$(now)
    emit "$metric.wall_ms" "$(ms "$s" "$e")"; emit "$metric.maxrss_kb" "$(cat "$WORK/time.out")"
    i=$((i + 1))
  done
}

new_project() {
  dir="$1"; ntokens="$2"
  rm -rf "$dir"; mkdir -p "$dir"
  git -C "$dir" init --quiet
  git -C "$dir" -c user.name=t -c user.email=t@example.com commit --quiet --allow-empty -m "chore: initial commit"
  printf '%s\n' 'schema_version = 1' '' '[nexora]' 'track = "never"' '' '[architect]' 'enabled = true' 'role = "developer"' '' \
    '[hosts.claude]' 'kind = "claude"' '' '[roles.pm]' 'kind = "PM"' 'host = "claude"' '' '[roles.developer]' 'kind = "Developer"' 'host = "claude"' >"$dir/capstan.toml"
  (cd "$dir" && XE=(); cs init >/dev/null)
  (cd "$dir" && node "$HERE/seed.mjs" "$WORK/npm3/package" "$dir" "$ntokens" >"$dir/tokens.json")
}
token() { node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))[process.argv[2]])' "$1" "$2"; }
as() { XE=(CAPSTAN_TOKEN="$(token "$P/tokens.json" "$1")" CAPSTAN_SOCKET="$SOCK" CAPSTAN_AGENT_ID="$1"); }
drain_inbox() { ids=$(cs inbox 2>/dev/null | awk '$1 == "message" { print $2 }'); for m in $ids; do cs ack "$m" >/dev/null 2>&1; done; }

small() {
  P="$WORK/$SIDE-small"; new_project "$P" 10; cd "$P"
  XE=(); s=$(now); cs --version >/dev/null; e=$(now); emit "cold.version_ms" "$(ms "$s" "$e")"
  s=$(now); cs start >/dev/null 2>&1; e=$(now); emit "daemon.first_start_ms" "$(ms "$s" "$e")"
  PID="$(cat "$P/.capstan/state/daemon.pid")"; SOCK="$P/.capstan/state/control.sock"
  # Cold start and stop, 5 cycles (the first start above opened the freshly seeded ledger).
  for c in 1 2 3 4 5; do
    XE=(); s=$(now); cs stop >/dev/null 2>&1; e=$(now); emit "daemon.stop_ms" "$(ms "$s" "$e")"
    s=$(now); cs start >/dev/null 2>&1; e=$(now); emit "daemon.cold_start_ms" "$(ms "$s" "$e")"
  done
  PID="$(cat "$P/.capstan/state/daemon.pid")"
  own "$PID" "$P" || { echo "bench: daemon $PID is not the scratch daemon" >&2; return 1; }
  sleep 5
  b=$(ticks "$PID"); sleep "$IDLE_SECONDS"; a=$(ticks "$PID")
  emit "idle.daemon_cpu_percent" "$(awk -v a="$b" -v b="$a" -v s="$IDLE_SECONDS" 'BEGIN { printf "%.3f", (b - a) / s }')"
  emit "idle.daemon_rss_kb" "$(kb "$PID" VmRSS)"; emit "idle.daemon_threads" "$(kb "$PID" Threads)"
  # CLI per call
  XE=(); timed "call.version" --version
  timed "call.ping" ping
  timed "call.config_check" config check
  as pm-1; timed "call.status" status --json
  as pm-1; timed "call.plan_show" plan show
  as pm-1; timed "call.inbox_empty" inbox
  as developer-1; timed "call.send" send pm-1 "benchmark message"
  as pm-1; drain_inbox
  # wait with nothing queued: blocked RSS after 3 s; then end it (our own child).
  as developer-1
  env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" "${CMD[@]}" wait >/dev/null 2>&1 & W=$!
  sleep 3
  cmd=$(tr '\0' ' ' </proc/$W/cmdline 2>/dev/null)
  case "$cmd" in *" wait "*|*" wait") emit "wait.blocked_rss_kb" "$(kb "$W" VmRSS)" ;; *) emit "wait.blocked_rss_kb" "NA" ;; esac
  kill "$W" 2>/dev/null; wait "$W" 2>/dev/null
  # 10 idle waiters (developer-1..10): sum of their RSS, plus the daemon's.
  WP=()
  for n in 1 2 3 4 5 6 7 8 9 10; do
    as "developer-$n"
    env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" "${CMD[@]}" wait >/dev/null 2>&1 & WP+=($!)
  done
  sleep 4; sum=0
  for w in "${WP[@]}"; do v=$(kb "$w" VmRSS); sum=$((sum + ${v:-0})); done
  emit "waiters10.sum_rss_kb" "$sum"; emit "waiters10.daemon_rss_kb" "$(kb "$PID" VmRSS)"
  for w in "${WP[@]}"; do kill "$w" 2>/dev/null; done; wait 2>/dev/null
  # Latency: pm waits, developer-1 sends; time from the send starting / returning to the wait returning.
  for i in 1 2 3 4 5 6 7 8 9 10; do
    as pm-1; rm -f "$WORK/wait.end"
    ( env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" timeout 30 "${CMD[@]}" wait >/dev/null 2>&1; now >"$WORK/wait.end" ) &
    LW=$!; sleep 1
    as developer-1; t0=$(now); cs send pm-1 "latency $i" >/dev/null 2>&1; t1=$(now)
    for k in $(seq 1 300); do [ -s "$WORK/wait.end" ] && break; sleep 0.05; done
    t2=$(cat "$WORK/wait.end" 2>/dev/null || echo "$t1"); wait "$LW" 2>/dev/null
    emit "latency.send_start_to_wait_return_ms" "$(ms "$t0" "$t2")"
    emit "latency.send_return_to_wait_return_ms" "$(awk -v a="$t1" -v b="$t2" 'BEGIN { printf "%.1f", (b - a) / 1e6 }')"
    as pm-1; drain_inbox
  done
  # Sequential throughput: N x (send, inbox, ack).
  N=60; s=$(now)
  for i in $(seq 1 $N); do
    as developer-1; cs send pm-1 "seq $i" >/dev/null 2>&1
    as pm-1; drain_inbox
  done
  e=$(now); emit "throughput.sequential_triplets_per_s" "$(awk -v n=$N -v a="$s" -v b="$e" 'BEGIN { printf "%.2f", n / ((b - a) / 1e9) }')"
  # Burst: 8 developers send 25 each concurrently; the daemon's peak RSS is reset first.
  echo 5 >"/proc/$PID/clear_refs" 2>/dev/null
  b=$(ticks "$PID"); s=$(now)
  for n in 1 2 3 4 5 6 7 8; do
    ( as "developer-$n"; for i in $(seq 1 25); do cs send pm-1 "burst $n $i" >/dev/null 2>&1; done ) &
  done
  wait; e=$(now); a=$(ticks "$PID")
  emit "burst.send_msgs_per_s" "$(awk -v a="$s" -v b="$e" 'BEGIN { printf "%.2f", 200 / ((b - a) / 1e9) }')"
  emit "burst.send_wall_ms" "$(ms "$s" "$e")"
  emit "burst.daemon_cpu_ms" "$(((a - b) * 10))"; emit "burst.daemon_peak_rss_kb" "$(kb "$PID" VmHWM)"
  # Burst ack: the pm reads the inbox once, then acks all 200 with 8 parallel acks.
  as pm-1; ids=$(cs inbox 2>/dev/null | awk '$1 == "message" { print $2 }'); emit "burst.inbox_count" "$(echo "$ids" | grep -c .)"
  echo 5 >"/proc/$PID/clear_refs" 2>/dev/null; b=$(ticks "$PID"); s=$(now)
  echo "$ids" | xargs -P 8 -I{} env -i HOME="$WORK/home" PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off "${XE[@]}" "${CMD[@]}" ack {} >/dev/null 2>&1
  e=$(now); a=$(ticks "$PID")
  emit "burst.ack_msgs_per_s" "$(awk -v a="$s" -v b="$e" 'BEGIN { printf "%.2f", 200 / ((b - a) / 1e9) }')"
  emit "burst.ack_daemon_cpu_ms" "$(((a - b) * 10))"; emit "burst.ack_daemon_peak_rss_kb" "$(kb "$PID" VmHWM)"
  emit "after_load.daemon_rss_kb" "$(kb "$PID" VmRSS)"; emit "after_load.ledger_bytes" "$(wc -c <"$P/.capstan/state/controller.sqlite")"
  # Dashboard (against this live daemon, 160x45 pty).
  if [ -n "$DASHBIN" ] && [ "$DASH_SECONDS" -gt 0 ]; then
    r=$(BENCH_DASH_CREDENTIAL="$(cat "$P/.capstan/operator.key")" BENCH_HOME="$WORK/home" python3 "$HERE/dash-sample.py" "$DASH_SECONDS" "$SOCK" "$DASHBIN" --socket "$SOCK" --interval 1 2>&1 | tail -1)
    emit "dash.cpu_percent" "$(echo "$r" | sed -n 's/.*cpu_percent=\([0-9.]*\).*/\1/p')"; emit "dash.rss_kb" "$(echo "$r" | sed -n 's/.*rss_kb=\([0-9]*\).*/\1/p')"
  fi
  # The 0.3.0 standalone also carries the Node (Ink) dashboard: `cstan dash` with CSTAN_DASH=node, run from the project root.
  if [ "$SIDE" = node ] && [ "$DASH_SECONDS" -gt 0 ]; then
    r=$(BENCH_X_CSTAN_DASH=node BENCH_HOME="$WORK/home" python3 "$HERE/dash-sample.py" "$DASH_SECONDS" "$SOCK" "${CMD[@]}" dash --interval 1 2>&1 | tail -1)
    emit "dash_node.cpu_percent" "$(echo "$r" | sed -n 's/.*cpu_percent=\([0-9.]*\).*/\1/p')"; emit "dash_node.rss_kb" "$(echo "$r" | sed -n 's/.*rss_kb=\([0-9]*\).*/\1/p')"
  fi
  XE=(); cs stop >/dev/null 2>&1
}

large() {
  P="$WORK/$SIDE-large"; rm -rf "$P"
  node "$WORK/genroot/scripts/shadow-daemon.mjs" --generate "$P" >/dev/null || return 1
  git -C "$P" init --quiet; git -C "$P" -c user.name=t -c user.email=t@example.com commit --quiet --allow-empty -m "chore: initial commit"
  cd "$P"; emit "large.ledger_bytes" "$(wc -c <"$P/.capstan/state/controller.sqlite")"
  XE=(); s=$(now); cs start >/dev/null 2>&1; e=$(now); emit "large.cold_start_ms" "$(ms "$s" "$e")"
  PID="$(cat "$P/.capstan/state/daemon.pid")"; SOCK="$P/.capstan/state/control.sock"
  own "$PID" "$P" || { echo "bench: daemon $PID is not the scratch daemon" >&2; return 1; }
  PM="$(token "$P/shadow-tokens.json" pm-1)"; XE=(CAPSTAN_TOKEN="$PM" CAPSTAN_SOCKET="$SOCK" CAPSTAN_AGENT_ID=pm-1)
  emit "large.daemon_rss_after_start_kb" "$(kb "$PID" VmRSS)"
  timed "large.status" status --json
  timed "large.inbox" inbox
  timed "large.plan_show" plan show
  emit "large.daemon_rss_after_calls_kb" "$(kb "$PID" VmRSS)"; emit "large.daemon_hwm_kb" "$(kb "$PID" VmHWM)"
  sleep 5; b=$(ticks "$PID"); sleep 30; a=$(ticks "$PID")
  emit "large.idle_daemon_cpu_percent" "$(awk -v a="$b" -v b="$a" 'BEGIN { printf "%.3f", (b - a) / 30 }')"
  XE=(); s=$(now); cs stop >/dev/null 2>&1; e=$(now); emit "large.stop_ms" "$(ms "$s" "$e")"
}

for ROUND in $(seq 1 "$ROUNDS"); do
  for SIDE in "${SIDES[@]}"; do
    setup_side "$SIDE"
    emit "machine.loadavg_1m" "$(cut -d' ' -f1 /proc/loadavg)"
    echo "round $ROUND side $SIDE: small" >&2; small
    emit "machine.loadavg_1m_after_small" "$(cut -d' ' -f1 /proc/loadavg)"
    echo "round $ROUND side $SIDE: large" >&2; large
    rm -rf "$WORK/$SIDE-small" "$WORK/$SIDE-large"
  done
done
echo "bench: done, $OUT" >&2
