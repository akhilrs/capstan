#!/bin/sh
# Baseline-vs-after protocol on a fresh copy, no profiler attached: protocol.sh <label> <dist dir>
# status latency over 200 calls, daemon CPU 3 x 60 s idle, then 3 x 60 s under status at 0.9/s, then memory at 10 minutes.
set -eu
S=${SCRATCH:-/tmp/capstan-prof}
here=$(cd "$(dirname "$0")" && pwd)
label=$1 dist=$2
out=$S/prof/$label; mkdir -p "$out"
"$here/reset.sh"
pid=$("$here/start.sh" "$dist")
t0=$(date +%s)
sleep 75
{
  echo "# $label $(date -Is) dist=$dist node=$(node -v)"
  node "$here/bench.mjs" latency status 200
  echo "idle:"; node "$here/bench.mjs" cpu "$pid" 60 3
  # The replay runs until 600 s after the start, so memory is read after 10 minutes under the replayed mix.
  left=$((600 - ($(date +%s) - t0)))
  node "$here/bench.mjs" replay 0.9 "$left" &
  load=$!
  sleep 3
  echo "under status 0.9/s:"; node "$here/bench.mjs" cpu "$pid" 60 3
  wait "$load"
  node "$here/bench.mjs" mem "$pid"
} > "$out/protocol.txt" 2>&1
kill -TERM "$pid"; sleep 5
cat "$out/protocol.txt"
