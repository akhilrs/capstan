#!/bin/sh
# Starts the scratch daemon: start.sh <dist dir> [node flags...]; prints the pid. Output goes to $SCRATCH/daemon.out.
S=${SCRATCH:-/tmp/capstan-prof}
dist=$1; shift
cd "$S/root" || exit 1
env -i PATH="$S/shims:/usr/bin:/bin" HOME="$S/home" LANG=C.UTF-8 TMPDIR="$S/tmp" MEMREPORT_OUT="${MEMREPORT_OUT:-/dev/null}" \
  "$(command -v node)" "$@" "$dist/src/cli.js" daemon > "$S/daemon.out" 2>&1 &
echo $! | tee "$S/daemon.pid"
