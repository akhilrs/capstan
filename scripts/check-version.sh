#!/bin/sh
# Fails unless VERSION, package.json and package-lock.json (top level and packages[""]) carry the same version.
# VERSION is the single source; package.json and the lock follow it until the npm files go away.
# Usage: scripts/check-version.sh [repository root]
set -eu

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"

# json_versions <file>: every "version" that sits at the top level (2 spaces) or in packages[""] (6 spaces) of the file.
json_versions() {
  awk '
    /^  "version": "/ { v = $0; sub(/^  "version": "/, "", v); sub(/",?$/, "", v); print v }
    /^    "": \{/ { inroot = 1; next }
    inroot && /^    \}/ { inroot = 0 }
    inroot && /^      "version": "/ { v = $0; sub(/^      "version": "/, "", v); sub(/",?$/, "", v); print v }
  ' "$1"
}

[ -f "$ROOT/VERSION" ] || { echo "check-version: $ROOT/VERSION is missing" >&2; exit 1; }
want="$(tr -d ' \t\r\n' <"$ROOT/VERSION")"
case "$want" in
  "" | *[!0-9.]* | .* | *. | *..*) echo "check-version: VERSION must be X.Y.Z, got '$want'" >&2; exit 1 ;;
esac
status=0
for file in package.json package-lock.json; do
  found="$(json_versions "$ROOT/$file" | tr '\n' ' ')"
  [ -n "$found" ] || { echo "check-version: no version found in $file" >&2; status=1; continue; }
  for v in $found; do
    if [ "$v" != "$want" ]; then
      echo "check-version: $file has version $v, VERSION has $want" >&2
      status=1
    fi
  done
done
[ "$status" -ne 0 ] || echo "check-version: $want in VERSION, package.json and package-lock.json"
exit "$status"
