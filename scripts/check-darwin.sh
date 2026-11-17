#!/bin/sh
# Checks that the workspace and the dashboard lint clean for macOS from a Linux host: cargo clippy --all-targets
# -D warnings for <arch>-apple-darwin, with zig cc as the C compiler.
# Needs `rustup target add <arch>-apple-darwin`, zig on PATH, and CARGO_TARGET_DIR outside the checkout.
# Honours CSTAN_CHECK_JOBS (cargo jobs).
# Usage: scripts/check-darwin.sh [--arch aarch64|x86_64]
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH=aarch64
while [ $# -gt 0 ]; do
  case "$1" in
    --arch)
      [ $# -ge 2 ] || { echo "--arch needs aarch64 or x86_64" >&2; exit 2; }
      ARCH="$2"
      shift 2
      ;;
    *)
      echo "usage: scripts/check-darwin.sh [--arch aarch64|x86_64]" >&2
      exit 2
      ;;
  esac
done
case "$ARCH" in aarch64 | x86_64) ;; *) echo "unknown arch: $ARCH" >&2; exit 2 ;; esac
TRIPLE="$ARCH-apple-darwin"

if ! command -v zig >/dev/null 2>&1; then
  echo "zig is not installed: install it (https://ziglang.org/download/) and put it on PATH" >&2
  exit 1
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "$TRIPLE"; then
  echo "the rust target $TRIPLE is not installed: run: rustup target add $TRIPLE" >&2
  exit 1
fi
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  echo "set CARGO_TARGET_DIR to a directory outside the checkout" >&2
  exit 1
fi
case "$CARGO_TARGET_DIR/" in "$ROOT"/*) echo "CARGO_TARGET_DIR must be outside the checkout" >&2; exit 1 ;; esac

WRAP="$(mktemp -d)"
trap 'rm -rf "$WRAP"' EXIT INT TERM
# cc-rs passes --target=<triple>, which zig does not accept: drop it and give zig its own target.
cat >"$WRAP/zigcc" <<WRAPPER
#!/bin/sh
args=""
for a in "\$@"; do
  case "\$a" in --target=*) ;; *) args="\$args '\$a'" ;; esac
done
eval "exec zig cc -target $ARCH-macos \$args"
WRAPPER
chmod +x "$WRAP/zigcc"
cat >"$WRAP/zigar" <<'WRAPPER'
#!/bin/sh
exec zig ar "$@"
WRAPPER
chmod +x "$WRAP/zigar"

UNDERSCORED="$(printf '%s' "$TRIPLE" | tr '-' '_')"
export "CC_$UNDERSCORED=$WRAP/zigcc" "AR_$UNDERSCORED=$WRAP/zigar"
JOBS="${CSTAN_CHECK_JOBS:-}"

FAILED=0
for dir in rust dash; do
  echo "== clippy $dir for $TRIPLE"
  if [ -n "$JOBS" ]; then
    (cd "$ROOT/$dir" && cargo clippy --keep-going --locked --all-targets --target "$TRIPLE" -j "$JOBS" -- -D warnings) || FAILED=1
  else
    (cd "$ROOT/$dir" && cargo clippy --keep-going --locked --all-targets --target "$TRIPLE" -- -D warnings) || FAILED=1
  fi
done
[ "$FAILED" -eq 0 ] && echo "ok   $TRIPLE clippy clean" || echo "FAIL $TRIPLE clippy"
exit "$FAILED"
