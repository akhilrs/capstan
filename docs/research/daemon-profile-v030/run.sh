#!/bin/sh
# One run on a fresh copy of the ledger: run.sh <label> <dist dir> <minutes> <profile|plain> [extra node flags...]
# Writes $SCRATCH/prof/<label>/: cpu.txt (own + children CPU per minute, herdr calls, threads, load), mem.txt, memreport.json,
# herdr-calls.txt (shim call counts by command; with RATE=<n> set, `status` is also called n times a second by the operator, as the live daemon.log shows), proc-start.txt/proc-end.txt (raw /proc/<pid>/stat and status), and with
# `profile` also cpu.cpuprofile (main thread) and heap.heapsnapshot. Idle: no client call is made during the run.
set -eu
S=${SCRATCH:-/tmp/capstan-prof-v030/s}
here=$(cd "$(dirname "$0")" && pwd)
base=$here/../daemon-profile
label=$1 dist=$2 minutes=$3 kind=$4; shift 4
out=$S/prof/$label; mkdir -p "$out"
"$base/reset.sh"
flags=""
[ "$kind" = profile ] && flags="--cpu-prof --cpu-prof-dir=$out --cpu-prof-name=cpu.cpuprofile --diagnostic-dir=$out --heapsnapshot-signal=SIGUSR2"
pid=$(MEMREPORT_OUT="$out/memreport.json" "$base/start.sh" "$dist" --import="$base/memreport.mjs" $flags "$@")
sleep 8
echo "pid $pid $(date -Is) load $(cut -d' ' -f1-3 /proc/loadavg) cwd $(readlink /proc/$pid/cwd)" > "$out/cpu.txt"
cat /proc/$pid/stat > "$out/proc-start.txt"
: > "$S/herdr-calls.log"
if [ -n "${RATE:-}" ]; then
  DIST=$dist node "$here/bench.mjs" replay "$RATE" $((minutes * 60)) > "$out/latency.txt" 2>&1 &
  loadgen=$!
fi
node "$here/bench.mjs" cpu "$pid" 60 "$minutes" >> "$out/cpu.txt" 2>&1
[ -z "${RATE:-}" ] || wait "$loadgen"
cat /proc/$pid/stat > "$out/proc-end.txt"; cat /proc/$pid/status > "$out/proc-status-end.txt"
node "$here/bench.mjs" mem "$pid" > "$out/mem.txt"
[ -z "${MEMSPLIT:-}" ] || node "$here/memsplit.mjs" "$pid" > "$out/memsplit.json"
awk '{ if ($1=="--session") { $1="";$2="" } print $1, $2 }' "$S/herdr-calls.log" | sort | uniq -c | sort -rn > "$out/herdr-calls.txt"
kill -URG "$pid"; sleep 3
if [ "$kind" = profile ]; then kill -USR2 "$pid"; sleep 25; fi
kill -TERM "$pid"; sleep 8
grep -v Warn "$out/cpu.txt" || true; cat "$out/mem.txt"; ls "$out"
