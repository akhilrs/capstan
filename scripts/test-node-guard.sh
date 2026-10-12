#!/bin/sh
# Tests the no-Node guard of scripts/check.sh: it passes on the real tree and fails when a scratch .ts file, a scratch
# package.json, an .mjs file or a tsconfig is added, and passes for a path under scripts/node-allowlist. The additions go into
# a temporary copy of the index (GIT_INDEX_FILE), so the working tree and the real index are never touched.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-test-node-guard.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail=0
ok() { echo "ok - $1"; }
bad() { echo "not ok - $1" >&2; fail=1; }

REAL_INDEX="$(git -C "$ROOT" rev-parse --path-format=absolute --git-path index)"
BLOB="$(git -C "$ROOT" hash-object -w --stdin </dev/null)"

guard() { sh "$ROOT/scripts/check.sh" node-guard >"$SANDBOX/out" 2>&1; }

# with_added <path>: runs the guard on a copy of the index that also lists <path>.
with_added() {
  cp "$REAL_INDEX" "$SANDBOX/index"
  GIT_INDEX_FILE="$SANDBOX/index" git -C "$ROOT" update-index --add --cacheinfo "100644,$BLOB,$1"
  GIT_INDEX_FILE="$SANDBOX/index" guard
}

if guard; then ok "the guard passes on the real tree"; else bad "the guard fails on the real tree"; cat "$SANDBOX/out" >&2; fi

for path in scratch-node-guard.ts scratch/dir/view.tsx scratch-node-guard.mjs package.json sub/package.json package-lock.json tsconfig.json eslint.config.js; do
  if with_added "$path"; then
    bad "the guard passed with $path added"
  elif grep -qF "$path" "$SANDBOX/out"; then
    ok "the guard fails on an added $path and names it"
  else
    bad "the guard failed on $path without naming it"; cat "$SANDBOX/out" >&2
  fi
done

if with_added docs/research/scratch-node-guard.mjs; then
  ok "the guard passes for a path under scripts/node-allowlist"
else
  bad "the guard failed for a path under scripts/node-allowlist"; cat "$SANDBOX/out" >&2
fi

[ "$fail" = 0 ] || exit 1
echo "test-node-guard: passed"
