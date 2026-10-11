#!/bin/sh
# Self-test of check-version.sh: builds scratch roots and shows each check failing on its own, then runs it on the real
# tree. Needs no network and no node.
# Usage: scripts/test-check-version.sh [repository root]
set -eu

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
CHECK="$ROOT/scripts/check-version.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAILURES=0

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}

# scratch <name> <VERSION content or -> <CHANGELOG content>: makes $TMP/<name>; "-" leaves VERSION out.
scratch() {
  mkdir -p "$TMP/$1"
  [ "$2" = "-" ] || printf '%s\n' "$2" >"$TMP/$1/VERSION"
  printf '%s\n' "$3" >"$TMP/$1/CHANGELOG.md"
}
expect_ok() {
  if sh "$CHECK" "$TMP/$1" >/dev/null 2>&1; then pass "$2"; else fail "$2"; fi
}
expect_fail() {
  if sh "$CHECK" "$TMP/$1" >/dev/null 2>&1; then fail "$2"; else pass "$2"; fi
}

GOOD="# Changelog

## 0.4.1 (2026-10-11)

- x"

scratch good 0.4.1 "$GOOD"
expect_ok good "matching VERSION and CHANGELOG pass"
scratch unreleased 0.4.1 "# Changelog

## Unreleased

- y

## 0.4.1 (2026-10-11)"
expect_ok unreleased "an Unreleased heading is skipped"
scratch short 0.4 "$GOOD"
expect_fail short "VERSION 0.4 fails"
scratch letter 0.4.x "$GOOD"
expect_fail letter "VERSION 0.4.x fails"
scratch empty "" "$GOOD"
expect_fail empty "empty VERSION fails"
scratch none - "$GOOD"
expect_fail none "missing VERSION fails"
scratch differs 0.4.1 "# Changelog

## 0.4.2 (2026-10-12)

## 0.4.1 (2026-10-11)"
expect_fail differs "first release heading differing from VERSION fails"
scratch nodate 0.4.1 "# Changelog

## 0.4.1"
expect_fail nodate "release heading without a date fails"
scratch noheading 0.4.1 "# Changelog"
expect_fail noheading "CHANGELOG without a release heading fails"

if sh "$CHECK" "$ROOT" >/dev/null 2>&1; then pass "check-version.sh passes on $ROOT"; else fail "check-version.sh passes on $ROOT"; fi

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES check-version test(s) failed"
  exit 1
fi
echo "check-version tests passed"
