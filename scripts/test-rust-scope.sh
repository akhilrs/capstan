#!/bin/sh
# Tests scripts/rust-scope.sh on a small fixture workspace (leaf <- mid <- top, plus solo) in a private git repository:
# a change in one leaf package gives that package only, a change in a dependency gives it and its dependents, and a
# change to Cargo.lock, rust/Cargo.toml, rust/.cargo or a build script, an unknown base, a missing base or an
# integration gives the full gate; a change outside rust/ gives the full gate unless the path is allowlisted (then none),
# and a file moved between packages gives both.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-test-rust-scope.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

command -v cargo >/dev/null 2>&1 || { echo "test-rust-scope: cargo not found" >&2; exit 1; }

fail=0
ok() { echo "ok - $1"; }
bad() { echo "not ok - $1 (got: $2)" >&2; fail=1; }

REPO="$SANDBOX/repo"
mkdir -p "$REPO/rust"
git -C "$REPO" init -q
git -C "$REPO" config user.email test@example.invalid
git -C "$REPO" config user.name test
git -C "$REPO" config commit.gpgsign false
cat >"$REPO/rust/Cargo.toml" <<'T'
[workspace]
resolver = "2"
members = ["crates/*"]
T
mk() { # name [dependency]
  mkdir -p "$REPO/rust/crates/$1/src"
  {
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' "$1"
    [ -z "${2:-}" ] || printf '%s = { path = "../%s" }\n' "$2" "$2"
  } >"$REPO/rust/crates/$1/Cargo.toml"
  echo "pub fn f() {}" >"$REPO/rust/crates/$1/src/lib.rs"
}
mk leaf
mk mid leaf
mk top mid
mk solo
echo "// extra" >"$REPO/rust/crates/leaf/src/extra.rs"
(cd "$REPO/rust" && cargo generate-lockfile --offline -q)
git -C "$REPO" add -A
git -C "$REPO" commit -q -m base
BASE="$(git -C "$REPO" rev-parse HEAD)"

# scope <subject> <base>: the answer of the script for the current HEAD
scope() {
  CAPSTAN_VERIFY_SUBJECT="$1" CAPSTAN_VERIFY_BASE="$2" sh "$ROOT/scripts/rust-scope.sh" "$REPO" 2>/dev/null
}
# change <file>: commits a change of one file on top of the base (resetting to the base first)
change() {
  git -C "$REPO" reset -q --hard "$BASE"
  mkdir -p "$(dirname "$REPO/$1")"
  echo "// change" >>"$REPO/$1"
  git -C "$REPO" add -A
  git -C "$REPO" commit -q -m change
}
expect() { # label expected actual
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "$3 (wanted $2)"; fi
}

change rust/crates/solo/src/lib.rs
expect "a change in a leaf package gives that package only" "packages: solo" "$(scope report "$BASE")"
change rust/crates/top/src/lib.rs
expect "a change in the top package gives the top package only" "packages: top" "$(scope report "$BASE")"
change rust/crates/leaf/src/lib.rs
expect "a change in a dependency gives it and its dependents" "packages: leaf mid top" "$(scope report "$BASE")"
change rust/Cargo.lock
expect "Cargo.lock gives the full gate" full "$(scope report "$BASE")"
change rust/Cargo.toml
expect "rust/Cargo.toml gives the full gate" full "$(scope report "$BASE")"
change rust/.cargo/config.toml
expect "rust/.cargo gives the full gate" full "$(scope report "$BASE")"
change rust/crates/solo/build.rs
expect "a build script gives the full gate" full "$(scope report "$BASE")"
change rust/crates/solo/src/lib.rs
expect "no base gives the full gate" full "$(scope report "")"
expect "an unknown base gives the full gate" full "$(scope report 0000000000000000000000000000000000000000)"
expect "an integration gives the full gate" full "$(scope integration "$BASE")"
expect "no subject gives the full gate" full "$(scope "" "$BASE")"
change migrations/0001.sql
expect "a migrations/ change gives the full gate" full "$(scope report "$BASE")"
change docs/reference/configuration.md
expect "docs/reference/configuration.md gives the full gate" full "$(scope report "$BASE")"
change VERSION
expect "VERSION gives the full gate" full "$(scope report "$BASE")"
change apps/ios/readme.txt
expect "an allowlisted path gives none" none "$(scope report "$BASE")"
change README.md
expect "an allowlisted root document gives none" none "$(scope report "$BASE")"
# a file moved between packages lists both the old and the new path
git -C "$REPO" reset -q --hard "$BASE"
git -C "$REPO" mv rust/crates/leaf/src/extra.rs rust/crates/solo/src/moved.rs
git -C "$REPO" commit -q -m move
expect "a move between packages gives both packages and their dependents" "packages: leaf mid solo top" "$(scope report "$BASE")"

[ "$fail" = 0 ] || exit 1
echo "test-rust-scope: passed"
