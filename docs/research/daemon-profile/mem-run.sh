#!/bin/sh
# Memory after N minutes with no profiler attached: mem-run.sh <label> <dist dir> <idle|load> [minutes=10]
# Writes $SCRATCH/prof/<label>/{mem.txt,memreport.json,latency.txt}: /proc RssAnon and the V8 / non-heap split.
set -eu
S=${SCRATCH:-/tmp/capstan-prof}
here=$(cd "$(dirname "$0")" && pwd)
label=$1 dist=$2 mode=$3 minutes=${4:-10}
out=$S/prof/$label; mkdir -p "$out"
"$here/reset.sh"
pid=$(MEMREPORT_OUT="$out/memreport.json" "$here/start.sh" "$dist" --import="$here/memreport.mjs")
sleep 8
secs=$((minutes * 60))
if [ "$mode" = load ]; then
  node "$here/bench.mjs" replay 0.9 "$secs" > "$out/latency.txt" &
  load=$!
fi
sleep "$secs"
node "$here/bench.mjs" mem "$pid" > "$out/mem.txt"
kill -URG "$pid"; sleep 3
if [ "$mode" = load ]; then wait "$load"; fi
kill -TERM "$pid"; sleep 5
cat "$out/mem.txt"
