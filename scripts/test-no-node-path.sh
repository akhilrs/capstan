#!/bin/sh
# Tests the CSTAN_NO_NODE=1 PATH scrub of scripts/check.sh: with a fake node and a fake tool in one PATH directory, the
# scrub keeps the tool reachable and drops node; and check.sh fails, naming node, when node still resolves.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/capstan-test-no-node.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail=0
ok() { echo "ok - $1"; }
bad() { echo "not ok - $1" >&2; fail=1; }

mkdir "$SANDBOX/mixed" "$SANDBOX/cwd"
for name in node npm npx tsc faketool; do
  printf '#!/bin/sh\necho fake %s\n' "$name" >"$SANDBOX/mixed/$name"
  chmod +x "$SANDBOX/mixed/$name"
done
# A system directory (git, sh, the coreutils). The fake git sits in the same directory as the fake node and fake tool: when
# check.sh runs it, it records what resolves inside the scrubbed environment, then runs the real git.
REAL_GIT="$(command -v git)"
BASE="$(dirname "$REAL_GIT"):$(dirname "$(command -v sh)")"
cat >"$SANDBOX/mixed/git" <<GIT
#!/bin/sh
for tool in faketool node npm npx tsc sh cat grep; do
  printf '%s=%s\n' "\$tool" "\$(command -v "\$tool" || true)"
done >"$SANDBOX/resolved"
printf 'PATH=%s\n' "\$PATH" >>"$SANDBOX/resolved"
exec "$REAL_GIT" "\$@"
GIT
chmod +x "$SANDBOX/mixed/git"

# 1. The scrub: faketool resolves, node and the other node tools do not, the log names the scrubbed directory.
out="$SANDBOX/out"
if PATH="$SANDBOX/mixed:$BASE" CSTAN_NO_NODE=1 sh "$ROOT/scripts/check.sh" test-map >"$out" 2>&1; then
  ok "check.sh test-map passes under CSTAN_NO_NODE=1"
else
  bad "check.sh test-map fails under CSTAN_NO_NODE=1"; cat "$out" >&2
fi
grep -q "^no-node: $SANDBOX/mixed -> " "$out" && ok "the log shows the scrubbed directory" || bad "the log does not show the scrubbed directory"
if [ -f "$SANDBOX/resolved" ]; then
  for tool in faketool sh cat grep; do
    [ -n "$(sed -n "s/^$tool=//p" "$SANDBOX/resolved")" ] && ok "$tool resolves after the scrub" || bad "$tool does not resolve after the scrub"
  done
  for tool in node npm npx tsc; do
    [ -z "$(sed -n "s/^$tool=//p" "$SANDBOX/resolved")" ] && ok "$tool does not resolve after the scrub" || bad "$tool still resolves"
  done
  grep -q "^PATH=.*$SANDBOX/mixed" "$SANDBOX/resolved" && bad "the scratch directory is still on PATH" || ok "the scratch directory left PATH"
else
  bad "the fake git did not run inside the scrubbed environment"
fi
mirror="$(sed -n 's/^no-node: .* -> \([^ ]*\) (.*/\1/p' "$out" | head -n 1)"
[ -n "$mirror" ] && [ ! -e "$mirror" ] && ok "the scratch mirror is removed on exit" || bad "the scratch mirror is still there"

# 2. check.sh fails, naming node, when node still resolves: an empty PATH entry is the working directory, which the scrub
# leaves as it is.
cp "$SANDBOX/mixed/node" "$SANDBOX/cwd/node"
if (cd "$SANDBOX/cwd" && PATH=":$BASE" CSTAN_NO_NODE=1 sh "$ROOT/scripts/check.sh" test-map >"$out" 2>&1); then
  bad "check.sh passed although node still resolves"
else
  grep -q 'node still resolves' "$out" && ok "check.sh fails and names node" || { bad "check.sh failed without naming node"; cat "$out" >&2; }
fi

[ "$fail" = 0 ] || exit 1
echo "test-no-node-path: passed"
