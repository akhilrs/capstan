#!/bin/sh
# Cuts a release locally: works out the next version from the Conventional Commits since the last v* tag, writes it into
# VERSION, package.json and package-lock.json, prepends the CHANGELOG.md section, commits `chore(release): v<version>`
# and creates the annotated tag v<version>. Nothing is built, pushed or published: pushing the tag starts
# .github/workflows/release.yml, which builds the musl binaries and writes the GitHub release.
#
# Usage: scripts/release.sh [--dry-run] [--version X.Y.Z]
#   --dry-run        Print the current and next version and the changelog section; change nothing.
#   --version X.Y.Z  Use this version instead of the computed one. It must be higher than the current version.
#
# The commit history is read by tools/release (a separate cargo workspace; needs cargo, or CSTAN_RELEASE_TOOL=<binary>).
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

die() {
  printf 'release: %s\n' "$*" >&2
  exit 1
}

DRY_RUN=0
WANTED=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --version)
      [ $# -ge 2 ] || die "--version needs X.Y.Z"
      WANTED="$2"
      shift
      ;;
    --version=*) WANTED="${1#*=}" ;;
    -h | --help)
      sed -n '2,/^set -eu/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

if [ -n "${WANTED}" ]; then
  case "$WANTED" in
    "" | *[!0-9.]* | .* | *. | *..*) die "--version needs X.Y.Z, got '$WANTED'" ;;
  esac
  [ "$(printf '%s' "$WANTED" | tr -cd . | wc -c | tr -d ' ')" = 2 ] || die "--version needs X.Y.Z, got '$WANTED'"
fi

git rev-parse --git-dir >/dev/null 2>&1 || die "not a git repository: $ROOT"
dirty="$(git status --porcelain --untracked-files=no)"
[ -z "$dirty" ] || die "tracked files have uncommitted changes:
$dirty"

# The release tool: CSTAN_RELEASE_TOOL, else built from tools/release.
if [ -n "${CSTAN_RELEASE_TOOL:-}" ]; then
  TOOL="$CSTAN_RELEASE_TOOL"
else
  command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust (https://rustup.rs)"
  cargo build --quiet --release --locked --manifest-path tools/release/Cargo.toml ||
    die "could not build tools/release"
  TOOL="${CARGO_TARGET_DIR:-$ROOT/tools/release/target}/release/cstan-release"
fi
[ -x "$TOOL" ] || die "release tool not found: $TOOL"

# Last-line key=value output of `plan`; its warnings go straight to stderr.
if [ -n "$WANTED" ]; then
  PLAN="$("$TOOL" plan --root "$ROOT" --version "$WANTED")" || exit 1
else
  PLAN="$("$TOOL" plan --root "$ROOT")" || exit 1
fi
plan_value() {
  printf '%s\n' "$PLAN" | sed -n "s/^$1=//p" | head -n 1
}
CURRENT="$(plan_value current)"
VERSION="$(plan_value next)"
LEVEL="$(plan_value level)"
COUNTED="$(plan_value counted)"
LISTED="$(plan_value listed)"
[ -n "$VERSION" ] || die "the release tool printed no next version"
TAG="v$VERSION"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  die "tag $TAG already exists"
fi

DATE="$(date -u +%Y-%m-%d)"
SECTION="$("$TOOL" section --root "$ROOT" --version "$VERSION" --date "$DATE")" || exit 1
printf 'current: %s\n' "$CURRENT"
printf 'next:    %s (%s)\n' "$VERSION" "$LEVEL"
printf 'commits: %s counted, %s listed (%s duplicate(s) collapsed)\n' "$COUNTED" "$LISTED" "$((COUNTED - LISTED))"
printf '\n%s\n' "$SECTION"

if [ "$DRY_RUN" = 1 ]; then
  echo "Assets the tag would build (see .github/workflows/release.yml):"
  for arch in x64 arm64; do echo "  cstan-$VERSION-linux-$arch"; done
  for arch in x64 arm64; do echo "  cstan-dash-$VERSION-linux-$arch"; done
  echo "  SHA256SUMS"
  echo "Dry run: nothing was changed."
  exit 0
fi

# bump_json <file>: sets the top-level "version" and packages[""].version, keeping the file byte for byte otherwise.
bump_json() {
  awk -v v="$VERSION" '
    /^  "version": "/ { sub(/"version": "[^"]*"/, "\"version\": \"" v "\"") }
    /^    "": \{/ { inroot = 1 }
    inroot && /^      "version": "/ { sub(/"version": "[^"]*"/, "\"version\": \"" v "\""); inroot = 0 }
    { print }
  ' "$1" >"$1.release.tmp" && mv "$1.release.tmp" "$1"
}

FILES="VERSION package.json package-lock.json CHANGELOG.md"
restore() {
  # shellcheck disable=SC2086
  git checkout -q HEAD -- $FILES 2>/dev/null || true
  rm -f package.json.release.tmp package-lock.json.release.tmp CHANGELOG.md.release.tmp
}
trap 'restore' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf '%s\n' "$VERSION" >VERSION
bump_json package.json
bump_json package-lock.json
{
  if [ -f CHANGELOG.md ]; then
    # Everything before the first "## " heading, then the new section, then the older sections.
    awk '/^## / { exit } { print }' CHANGELOG.md
  else
    printf '# Changelog\n\n'
  fi
  printf '%s\n' "$SECTION"
  if [ -f CHANGELOG.md ]; then awk 'found || /^## / { found = 1; print }' CHANGELOG.md; fi
} >CHANGELOG.md.release.tmp
mv CHANGELOG.md.release.tmp CHANGELOG.md

sh "$ROOT/scripts/check-version.sh" "$ROOT" >/dev/null || die "the bump left VERSION, package.json and package-lock.json out of step"

if git add $FILES && git commit -q -m "chore(release): $TAG" && git tag -a "$TAG" -m "$TAG"; then
  trap - EXIT
else
  trap - EXIT
  printf 'release: committing or tagging failed. To undo it, run:\n  git tag -d %s\n  git reset --hard HEAD~1   # only if the chore(release) commit was created\n' "$TAG" >&2
  exit 1
fi
printf 'committed and tagged %s (local only; nothing was pushed)\n\n' "$TAG"
echo "To publish, push the commit and the tag; the Release workflow builds and publishes the assets:"
echo "  git push origin HEAD $TAG"
