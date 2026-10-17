#!/bin/sh
# Tests that the cargo gate of scripts/check.sh builds without incremental compilation (CARGO_INCREMENTAL=0) unless the
# caller sets CARGO_INCREMENTAL. A fake cargo records the variable and fails, which stops the gate at its first step.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-test-gate-env.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail=0
ok() { echo "ok - $1"; }
bad() { echo "not ok - $1" >&2; fail=1; }

mkdir "$SANDBOX/bin"
cat >"$SANDBOX/bin/cargo" <<CARGO
#!/bin/sh
printf '%s' "\${CARGO_INCREMENTAL-unset}" >"$SANDBOX/seen"
exit 1
CARGO
chmod +x "$SANDBOX/bin/cargo"

seen_with() {
  rm -f "$SANDBOX/seen"
  if [ "$1" = unset ]; then
    env -u CARGO_INCREMENTAL PATH="$SANDBOX/bin:$PATH" sh "$ROOT/scripts/check.sh" rust >/dev/null 2>&1 || true
  else
    env CARGO_INCREMENTAL="$1" PATH="$SANDBOX/bin:$PATH" sh "$ROOT/scripts/check.sh" rust >/dev/null 2>&1 || true
  fi
  cat "$SANDBOX/seen" 2>/dev/null || echo "no-cargo-run"
}

[ "$(seen_with unset)" = 0 ] && ok "the gate defaults to CARGO_INCREMENTAL=0" || bad "the gate does not default to CARGO_INCREMENTAL=0"
[ "$(seen_with 1)" = 1 ] && ok "an explicit CARGO_INCREMENTAL=1 wins" || bad "an explicit CARGO_INCREMENTAL=1 is overridden"

exit "$fail"
