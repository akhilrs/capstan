#!/bin/sh
# Fails unless VERSION is X.Y.Z and the first release heading of CHANGELOG.md ("## X.Y.Z (YYYY-MM-DD)") carries the same
# version. An "## Unreleased" heading is skipped. VERSION is the single source.
# Usage: scripts/check-version.sh [repository root]
set -eu

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"

[ -f "$ROOT/VERSION" ] || { echo "check-version: $ROOT/VERSION is missing" >&2; exit 1; }
want="$(tr -d ' \t\r\n' <"$ROOT/VERSION")"
case "$want" in
  "" | *[!0-9.]* | .* | *. | *..*) echo "check-version: VERSION must be X.Y.Z, got '$want'" >&2; exit 1 ;;
esac
[ "$(printf '%s' "$want" | tr -cd . | wc -c | tr -d ' ')" = 2 ] || { echo "check-version: VERSION must be X.Y.Z, got '$want'" >&2; exit 1; }

[ -f "$ROOT/CHANGELOG.md" ] || { echo "check-version: $ROOT/CHANGELOG.md is missing" >&2; exit 1; }
heading="$(grep -E '^## ' "$ROOT/CHANGELOG.md" | grep -v -i -E '^## +Unreleased' | head -n 1 || true)"
found="$(printf '%s\n' "$heading" | sed -n 's/^## \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\) ([0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\})$/\1/p')"
[ -n "$found" ] || { echo "check-version: first CHANGELOG.md release heading must be '## X.Y.Z (YYYY-MM-DD)', got '$heading'" >&2; exit 1; }
if [ "$found" != "$want" ]; then
  echo "check-version: CHANGELOG.md starts with $found, VERSION has $want" >&2
  exit 1
fi
echo "check-version: $want in VERSION and CHANGELOG.md"
