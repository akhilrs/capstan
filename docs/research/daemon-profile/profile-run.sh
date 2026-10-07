#!/bin/sh
# One profiling run on a fresh copy: profile-run.sh <label> <dist dir> <idle|load> [minutes=10]
# Writes $SCRATCH/prof/<label>/{cpu.cpuprofile,heap.heapsnapshot,mem.txt,cpu.txt,latency.txt}.
# load = status replayed at 0.9/s (the live rate seen in daemon.log) for the whole run.
set -eu
S=${SCRATCH:-/tmp/capstan-prof}
here=$(cd "$(dirname "$0")" && pwd)
label=$1 dist=$2 mode=$3 minutes=${4:-10}
out=$S/prof/$label; mkdir -p "$out"
"$here/reset.sh"
pid=$(MEMREPORT_OUT="$out/memreport.json" "$here/start.sh" "$dist" --import="$here/memreport.mjs" --cpu-prof --cpu-prof-dir="$out" --cpu-prof-name=cpu.cpuprofile --diagnostic-dir="$out" --heapsnapshot-signal=SIGUSR2)
sleep 8
secs=$((minutes * 60))
if [ "$mode" = load ]; then
  node "$here/bench.mjs" replay 0.9 "$secs" > "$out/latency.txt" &
  load=$!
fi
node "$here/bench.mjs" cpu "$pid" "$secs" 1 > "$out/cpu.txt"
node "$here/bench.mjs" mem "$pid" > "$out/mem.txt"
kill -URG "$pid"; sleep 3
if [ "$mode" = load ]; then wait "$load"; fi
# The snapshot blocks the event loop, so it is taken after the load has stopped.
kill -USR2 "$pid"; sleep 25
kill -TERM "$pid"; sleep 10
ls "$out"
