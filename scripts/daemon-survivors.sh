#!/bin/sh
# Lists the daemon processes that were started from a cargo target directory, one "pid<TAB>cwd<TAB>command" line each, so a test
# run can be checked for daemons it left behind (the tests start `cstan daemon` and must stop every one).
# Usage: scripts/daemon-survivors.sh list <target-dir>
#        scripts/daemon-survivors.sh check <target-dir> <command> [args...]
#   list    prints the daemons whose executable is inside <target-dir>
#   check   lists them, runs the command, waits up to 10 s for daemons that are shutting down, and fails (exit 1, the leaked
#           daemons on stderr) if a daemon is alive that was not in the first listing; the command's own failure is returned
#           when it fails. Nothing is signalled: the leaked processes are left for the person to inspect.
# A daemon is a process whose executable is inside the target directory (a binary replaced or removed since it started still
# counts) and that has a "daemon" argument or is named like one (cstan daemon, cstan-daemon, client-test-daemon). Linux /proc only;
# elsewhere the check is skipped with a note.
set -eu

list() {
  dir="$(cd "$1" 2>/dev/null && pwd -P)" || return 0
  [ -d /proc/self ] || return 0
  for proc in /proc/[0-9]*; do
    exe="$(readlink "$proc/exe" 2>/dev/null)" || continue
    exe="${exe% (deleted)}"
    case "$exe" in "$dir"/*) ;; *) continue ;; esac
    cmd="$(tr '\0' ' ' <"$proc/cmdline" 2>/dev/null)" || continue
    case "$(basename "$exe") $cmd" in *daemon*) ;; *) continue ;; esac
    printf '%s\t%s\t%s\n' "${proc#/proc/}" "$(readlink "$proc/cwd" 2>/dev/null || echo '?')" "$cmd"
  done
}

pids() { cut -f1 | sort; }

case "${1:-}" in
  list) [ $# = 2 ] || { echo "usage: $0 list <target-dir>" >&2; exit 2; }; list "$2" ;;
  check)
    [ $# -ge 3 ] || { echo "usage: $0 check <target-dir> <command> [args...]" >&2; exit 2; }
    dir="$2"; shift 2
    if [ ! -d /proc/self ]; then echo "daemon-survivors: no /proc here, the check is skipped" >&2; exec "$@"; fi
    before="$(list "$dir" | pids)"
    status=0
    "$@" || status=$?
    tries=0
    while :; do
      after="$(list "$dir")"
      new="$(printf '%s\n' "$after" | pids | grep -vxF -e "$before" -e '' || true)"
      [ -n "$new" ] || break
      tries=$((tries + 1))
      [ "$tries" -le 20 ] || break
      sleep 0.5
    done
    if [ -n "$new" ]; then
      echo "daemon-survivors: the run left daemons behind (stop them with kill; pid, cwd, command):" >&2
      for pid in $new; do
        printf '%s\n' "$after" | awk -F'\t' -v pid="$pid" '$1 == pid' >&2
      done
      exit 1
    fi
    exit "$status"
    ;;
  *) echo "usage: $0 list|check <target-dir> ..." >&2; exit 2 ;;
esac
