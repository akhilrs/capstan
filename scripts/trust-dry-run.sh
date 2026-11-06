#!/usr/bin/env bash
# Runs `cargo test` with the trust gate switched on and a private trust store, so a test that writes a capstan.toml shows
# up when the gate would refuse it. Builds the cstan binary from this tree first, so no stale build is used.
# Usage: scripts/trust-dry-run.sh <cargo test arguments>
set -euo pipefail

if [ -n "${CAPSTAN_TRUST_DRY_RUN+x}" ]; then
  echo "trust-dry-run: CAPSTAN_TRUST_DRY_RUN is already set; refusing to run inside a dry run" >&2
  exit 2
fi
export CAPSTAN_TRUST_DRY_RUN=1

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rust="$root/rust"

dir="$(mktemp -d)"
chmod 700 "$dir"
trap 'rm -rf "$dir"' EXIT

target="${CARGO_TARGET_DIR:-$rust/target}"
case "$target" in
  /*) ;;
  *) target="$rust/$target" ;;
esac

cd "$rust"
cargo build -p cstan-front
bin="$target/debug/cstan"
if [ ! -x "$bin" ]; then
  echo "trust-dry-run: $bin is missing after the build" >&2
  exit 1
fi

CSTAN_BIN="$bin" CAPSTAN_TRUST_DIR="$dir" CAPSTAN_TRUST_ENFORCE=1 cargo test --no-fail-fast "$@"
