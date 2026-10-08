#!/bin/sh
# The final measurement, run inside a PM-granted quiet window: final.sh <baseline dist> <after dist> [repeats=3] [minutes=10] [first number=1]
# Interleaves baseline and after (base1, after1, base2, ...), each a 10-minute idle run on a fresh copy of the ledger with the replay
# shim and no profiler. The baseline starts as v0.3.0 does (no node flags); the after run starts as the fixed build's daemon spawn
# does (--max-semi-space-size=2, see src/client.ts). Writes $SCRATCH/prof/final-<label>/ and a line with the load averages per run.
set -eu
S=${SCRATCH:-/tmp/capstan-prof-v030/s}
here=$(cd "$(dirname "$0")" && pwd)
base=$1 after=$2 repeats=${3:-3} minutes=${4:-10} first=${5:-1}
i=$first
last=$((first + repeats - 1))
while [ "$i" -le "$last" ]; do
  echo "=== final-base-$i start $(date -Is) load $(cut -d' ' -f1-3 /proc/loadavg)"
  sh "$here/run.sh" "final-base-$i" "$base" "$minutes" plain
  echo "=== final-base-$i end $(date -Is) load $(cut -d' ' -f1-3 /proc/loadavg)"
  echo "=== final-after-$i start $(date -Is) load $(cut -d' ' -f1-3 /proc/loadavg)"
  sh "$here/run.sh" "final-after-$i" "$after" "$minutes" plain --max-semi-space-size=2
  echo "=== final-after-$i end $(date -Is) load $(cut -d' ' -f1-3 /proc/loadavg)"
  i=$((i + 1))
done
