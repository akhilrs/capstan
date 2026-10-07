#!/bin/sh
# Restores the pristine ledger copy into the scratch project and removes the previous run's files.
# Usage: reset.sh   (SCRATCH defaults to /tmp/capstan-prof). Refuses a running scratch daemon.
S=${SCRATCH:-/tmp/capstan-prof}
state=$S/root/.capstan/state
[ -S "$state/control.sock" ] && { echo "a daemon is running; stop it first" >&2; exit 1; }
rm -f "$state"/controller.sqlite* "$state"/daemon.pid "$state"/notifications.jsonl "$state"/control.sock
cp "$S/pristine.sqlite" "$state/controller.sqlite"; chmod 600 "$state/controller.sqlite"
: > "$S/herdr-calls.log"; rm -f "$S/git-calls.log" "$S/git-refused.log"
