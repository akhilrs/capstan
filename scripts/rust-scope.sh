#!/bin/sh
# Which part of the rust/ workspace a report gate has to test.
#   scripts/rust-scope.sh [repo-root]
# Prints one line on stdout: "full" (run the whole workspace gate), "none" (nothing under rust/ changed) or
# "packages: a b c" (the changed workspace packages and every workspace package that depends on them). The reason goes to
# stderr. Only CAPSTAN_VERIFY_SUBJECT=report with a CAPSTAN_VERIFY_BASE commit narrows the scope; with anything else, or
# when the set cannot be computed (the base is unknown, cargo cannot list the packages, a change outside rust/ other than an allowlisted path no Rust code
# reads, a change outside the package
# directories such as rust/Cargo.toml, rust/Cargo.lock or rust/.cargo, or a build script), the answer is "full".
# Needs cargo and git only.
set -u

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"

full() { echo "scope: full workspace gate ($1)" >&2; echo full; exit 0; }

[ "${CAPSTAN_VERIFY_SUBJECT:-}" = report ] || full "not a report gate"
BASE="${CAPSTAN_VERIFY_BASE:-}"
[ -n "$BASE" ] || full "no CAPSTAN_VERIFY_BASE"
git -C "$ROOT" cat-file -e "$BASE^{commit}" 2>/dev/null || full "base $BASE is not a commit here"
changed="$(git -C "$ROOT" -c core.quotepath=off diff --name-only --no-renames "$BASE...HEAD" 2>/dev/null)" || full "git diff failed"

# Paths outside rust/ that no Rust build or test reads (checked with a grep of the Rust tree for ../ paths, include_str!,
# include_bytes! and repo-root reads): the mobile and desktop apps, the site, the decision records, the design sources and
# the plain root documents. Every other path outside rust/ (migrations/, docs/, scripts/, VERSION, dash/, tools/ ...) is read
# by some build script or test, so a change there, or to a path not listed here, gives the full gate.
outside_unused() {
  case "$1" in
    apps/* | site/* | decisions/* | design/* | CHANGELOG.md | LICENSE | README.md) return 0 ;;
  esac
  return 1
}

rust_changed=""
while IFS= read -r path; do
  [ -n "$path" ] || continue
  case "$path" in
    rust/*) rust_changed="$rust_changed
$path" ;;
    *) outside_unused "$path" || full "$path is outside rust/ and may be read by the Rust build or tests" ;;
  esac
done <<EOF
$changed
EOF
if [ -z "$rust_changed" ]; then
  echo "scope: nothing under rust/ changed since $BASE (only paths no Rust code reads)" >&2
  echo none
  exit 0
fi

# Workspace members: "name version (/abs/dir)" lines of a depth-0 tree.
members="$(cd "$ROOT/rust" && cargo tree --offline --workspace --depth 0 --prefix none --format '{p}' 2>/dev/null)" ||
  full "cargo tree failed"
[ -n "$members" ] || full "cargo lists no workspace package"
member_table="$(printf '%s\n' "$members" | sed -n 's|^\([^ ]*\) [^ ]* (\(/[^)]*\)).*$|\1 \2|p' | sort -u)"
[ -n "$member_table" ] || full "workspace packages have no path"

set_names=""
while IFS= read -r path; do
  [ -n "$path" ] || continue
  case "$path" in
    */build.rs | */build/*) full "build script $path" ;;
  esac
  abs="$ROOT/$path"
  hit=""
  best=0
  while read -r name dir; do
    case "$abs" in
      "$dir"/*)
        if [ "${#dir}" -gt "$best" ]; then best="${#dir}"; hit="$name"; fi
        ;;
    esac
  done <<EOF
$member_table
EOF
  [ -n "$hit" ] || full "$path belongs to no workspace package"
  case " $set_names " in *" $hit "*) ;; *) set_names="$set_names $hit" ;; esac
done <<EOF
$rust_changed
EOF

# Every workspace package that depends on a changed one (normal, build and dev edges).
all="$set_names"
for name in $set_names; do
  dependents="$(cd "$ROOT/rust" && cargo tree --offline -i "$name" -e all --prefix none --format '{p}' 2>/dev/null)" ||
    full "cargo tree -i $name failed"
  for dep in $(printf '%s\n' "$dependents" | sed -n 's|^\([^ ]*\) [^ ]* (/.*$|\1|p'); do
    printf '%s\n' "$member_table" | grep -q "^$dep " || continue
    case " $all " in *" $dep "*) ;; *) all="$all $dep" ;; esac
  done
done
all="$(printf '%s\n' $all | sort -u | tr '\n' ' ')"
all="${all% }"
echo "scope: packages $all (changed since $BASE, with their dependents)" >&2
echo "packages: $all"
