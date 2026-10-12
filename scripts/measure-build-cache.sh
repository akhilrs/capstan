#!/usr/bin/env bash
# Measure a cold and a warm build of any project, with or without a shared cache.
#
#   scripts/measure-build-cache.sh <repo> <setup-cmd> <build-cmd> [--cache-env <file>] [--reuse <path>]...
#
# Two fresh git worktrees of <repo> (at HEAD) are created. In the first, <setup-cmd> and <build-cmd> run with an empty
# cache root (cold). In the second they run with the same root, now filled by the first run (warm). Both commands run
# with `bash -c` from the worktree root. The script prints the wall time of each step, the cache size and, when the
# cache variables include SCCACHE_DIR and sccache is installed, its hit statistics. Its worktrees and temp root are
# removed on exit, also after a failure or an interrupt.
#
# --cache-env <file>   Lines NAME=value (blank lines and # comments are skipped). $CACHE_ROOT in a value is replaced by
#                      the cache root of the run. Without the option no cache variable is set (the "no cache" row).
# --reuse <path>       Setup reuse: in the warm worktree, copy <path> (relative to the worktree root, repeatable) from
#                      the cold worktree instead of running <setup-cmd>, the way a restored setup snapshot would.
#
# Example (Node, with the node preset's variables):
#   printf 'NPM_CONFIG_CACHE=$CACHE_ROOT/npm\n' > /tmp/node.env
#   scripts/measure-build-cache.sh . 'npm ci' 'npm run build' --cache-env /tmp/node.env
set -euo pipefail

usage() {
  sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ "$#" -ge 3 ] || usage
repo=$1
setup_cmd=$2
build_cmd=$3
shift 3

cache_env_file=
reuse_paths=()
while [ "$#" -gt 0 ]; do
  case $1 in
    --cache-env)
      [ "$#" -ge 2 ] || usage
      cache_env_file=$2
      shift 2
      ;;
    --reuse)
      [ "$#" -ge 2 ] || usage
      reuse_paths+=("$2")
      shift 2
      ;;
    *) usage ;;
  esac
done

repo=$(cd "$repo" && git rev-parse --show-toplevel)
[ -z "$cache_env_file" ] || [ -r "$cache_env_file" ] || {
  echo "cannot read $cache_env_file" >&2
  exit 2
}
for path in ${reuse_paths[@]+"${reuse_paths[@]}"}; do
  case $path in
    /* | *..*) echo "--reuse path must be relative without ..: $path" >&2; exit 2 ;;
  esac
done

tmp=$(mktemp -d "${TMPDIR:-/tmp}/measure-build-cache.XXXXXX")
root="$tmp/cache"
worktrees=()
sccache_port=

cleanup() {
  status=$?
  trap - EXIT INT TERM
  stop_sccache
  for worktree in ${worktrees[@]+"${worktrees[@]}"}; do
    git -C "$repo" worktree remove --force "$worktree" >/dev/null 2>&1 || true
  done
  git -C "$repo" worktree prune >/dev/null 2>&1 || true
  rm -rf -- "$tmp"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The cache variables of one run, as NAME=value lines on stdout.
cache_vars() {
  [ -n "$cache_env_file" ] || return 0
  local line name value
  while IFS= read -r line || [ -n "$line" ]; do
    case $line in '' | '#'*) continue ;; esac
    name=${line%%=*}
    value=${line#*=}
    [[ $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || {
      echo "bad line in $cache_env_file: $line" >&2
      return 1
    }
    printf '%s=%s\n' "$name" "${value//\$CACHE_ROOT/$root}"
  done <"$cache_env_file"
}

uses_sccache() {
  [ -n "$cache_env_file" ] && grep -q '^SCCACHE_DIR=' "$cache_env_file" && command -v sccache >/dev/null 2>&1
}

# sccache runs its own server on a private port, so a server of the user's is never touched.
stop_sccache() {
  [ -n "$sccache_port" ] || return 0
  SCCACHE_SERVER_PORT=$sccache_port sccache --stop-server >/dev/null 2>&1 || true
  sccache_port=
}

now() { date +%s.%N; }
elapsed() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", b - a }'; }
kib() { du -sk -- "$1" 2>/dev/null | cut -f1; }
human() { awk -v k="${1:-0}" 'BEGIN { if (k >= 1048576) printf "%.2f GiB", k / 1048576; else printf "%.1f MiB", k / 1024 }'; }

# run <label> <worktree> <step-label> <command>: runs one command with the run's cache variables; sets STEP_SECONDS.
run_step() {
  local worktree=$1 label=$2 command=$3 start end
  local env_args=()
  local line
  while IFS= read -r line; do
    [ -z "$line" ] || env_args+=("$line")
  done < <(cache_vars)
  [ -z "$sccache_port" ] || env_args+=("SCCACHE_SERVER_PORT=$sccache_port")
  start=$(now)
  if ! (cd "$worktree" && env ${env_args[@]+"${env_args[@]}"} bash -c "$command") >"$tmp/$label.log" 2>&1; then
    echo "FAILED: $label (log follows)" >&2
    tail -n 40 "$tmp/$label.log" >&2
    exit 1
  fi
  end=$(now)
  STEP_SECONDS=$(elapsed "$start" "$end")
}

sccache_zero() {
  uses_sccache || return 0
  stop_sccache
  sccache_port=$((20000 + RANDOM % 20000))
  SCCACHE_SERVER_PORT=$sccache_port SCCACHE_DIR="$root/sccache" sccache --zero-stats >/dev/null 2>&1 || true
}

sccache_report() {
  uses_sccache || return 0
  SCCACHE_SERVER_PORT=$sccache_port SCCACHE_DIR="$root/sccache" sccache --show-stats 2>&1 |
    grep -E '^(Compile requests executed|Cache hits|Cache misses|Cache hits rate|Cache location|Cache size)' |
    sed 's/^/    sccache: /'
}

# phase <name> <worktree> <reuse-from>: setup, build, then the sizes.
phase() {
  local name=$1 worktree=$2 from=${3:-} setup build before_kib
  sccache_zero
  if [ -n "$from" ] && [ "${#reuse_paths[@]}" -gt 0 ]; then
    local start end path
    start=$(now)
    for path in "${reuse_paths[@]}"; do
      [ -e "$from/$path" ] || continue
      mkdir -p "$(dirname "$worktree/$path")"
      cp -a "$from/$path" "$worktree/$path"
    done
    end=$(now)
    setup=$(elapsed "$start" "$end")
    setup="$setup (copied: ${reuse_paths[*]})"
  else
    run_step "$worktree" "$name-setup" "$setup_cmd"
    setup=$STEP_SECONDS
  fi
  run_step "$worktree" "$name-build" "$build_cmd"
  build=$STEP_SECONDS
  before_kib=$(kib "$root")
  echo "$name:"
  echo "    setup: ${setup} s"
  echo "    build: ${build} s"
  echo "    worktree size: $(human "$(kib "$worktree")")"
  echo "    cache size after this run: $(human "${before_kib:-0}")"
  sccache_report
}

git -C "$repo" worktree add --detach "$tmp/cold" HEAD >/dev/null 2>&1
worktrees+=("$tmp/cold")
git -C "$repo" worktree add --detach "$tmp/warm" HEAD >/dev/null 2>&1
worktrees+=("$tmp/warm")
mkdir -p "$root"

echo "repo: $repo ($(git -C "$repo" rev-parse --short HEAD))"
echo "setup: $setup_cmd"
echo "build: $build_cmd"
echo "cache env: ${cache_env_file:-none}"
phase cold "$tmp/cold"
phase warm "$tmp/warm" "$tmp/cold"
echo "cleanup: removing worktrees and $tmp"
