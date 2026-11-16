#!/bin/sh
# Checks .github/workflows/release.yml. Without options: actionlint when it is installed, else a parser check
# (python3 with PyYAML), then asserts the release rules:
#   - no setup-node, npm or node step; no tgz, cstan-front, cstan-node or cstan-daemon asset
#   - the four binaries are built with cargo-zigbuild for x86_64- and aarch64-unknown-linux-musl
#   - an x64 job and a job on the native ubuntu-24.04-arm runner each run `cstan --version` and `cstan-dash --version`
#   - the publish job needs both of those jobs and writes SHA256SUMS
#   - a build-macos job (matrix macos-15 arm64 and macos-15-intel x64) builds, ad-hoc signs, checks the architecture of
#     and runs the four macOS assets; publish needs it, runs only on a push, and lists all eight assets
#   - workflow_dispatch is a trigger (dry run) and only the version step's tag compare reads GITHUB_REF_NAME
# The macOS assets are checked only by the workflow itself on GitHub; --build below is Linux-only (zigbuild).
# CAPSTAN_RELEASE_WORKFLOW overrides the workflow path, so a mutated copy can be checked.
# With --build it also builds both triples locally with cargo-zigbuild (set CARGO_TARGET_DIR outside the checkout),
# checks every binary with `file` and runs the arm64 pair under qemu-aarch64 when that is installed.
# Usage: scripts/check-release-workflow.sh [--build]
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKFLOW="${CAPSTAN_RELEASE_WORKFLOW:-$ROOT/.github/workflows/release.yml}"
FAILURES=0

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}
has() { grep -q -- "$1" "$WORKFLOW"; }

[ -f "$WORKFLOW" ] || { echo "missing $WORKFLOW" >&2; exit 1; }

# --- lint or parse ------------------------------------------------------------
if command -v actionlint >/dev/null 2>&1; then
  if actionlint "$WORKFLOW"; then pass "actionlint"; else fail "actionlint"; fi
elif python3 -I -c 'import yaml' >/dev/null 2>&1; then
  if python3 -I -c 'import sys, yaml; yaml.safe_load(open(sys.argv[1]))' "$WORKFLOW"; then
    pass "parses as YAML (actionlint is not installed)"
  else
    fail "parses as YAML"
  fi
else
  fail "neither actionlint nor python3 with PyYAML is installed, so the workflow was not parsed"
fi

# --- structure, read from the parsed file --------------------------------------
if python3 -I -c 'import yaml' >/dev/null 2>&1; then
  STRUCT="$(python3 -I - "$WORKFLOW" <<'PY'
import sys, yaml

doc = yaml.safe_load(open(sys.argv[1]))
jobs = doc["jobs"]
out = []


def steps(job):
    return jobs[job].get("steps", [])


def runs(job):
    return "\n".join(s.get("run", "") for s in steps(job))


def need(job):
    n = jobs[job].get("needs", [])
    return [n] if isinstance(n, str) else list(n)


x64 = [j for j, d in jobs.items() if str(d.get("runs-on")) == "ubuntu-latest" and "cstan-dash-$VERSION-linux-x64" in runs(j) and "--version" in runs(j)]
arm = [j for j, d in jobs.items() if str(d.get("runs-on")) == "ubuntu-24.04-arm" and "cstan-dash-$VERSION-linux-arm64" in runs(j) and "--version" in runs(j)]
out.append(("an x64 job runs cstan --version and cstan-dash --version", len(x64) == 1 and "cstan-$VERSION-linux-x64 --version" in runs(x64[0])))
out.append(("a job on ubuntu-24.04-arm runs cstan --version and cstan-dash --version", len(arm) == 1 and "cstan-$VERSION-linux-arm64 --version" in runs(arm[0])))
pub = [j for j in jobs if "SHA256SUMS" in runs(j) and "gh release create" in runs(j)]
out.append(("one publish job writes SHA256SUMS", len(pub) == 1))
if len(pub) == 1 and len(x64) == 1 and len(arm) == 1:
    out.append(("the publish job needs both verify jobs", {x64[0], arm[0]} <= set(need(pub[0]))))
else:
    out.append(("the publish job needs both verify jobs", False))

mac = jobs.get("build-macos")
inc = []
if mac:
    inc = (mac.get("strategy") or {}).get("matrix", {}).get("include", [])
want_inc = [
    {"arch": "arm64", "runner": "macos-15", "triple": "aarch64-apple-darwin"},
    {"arch": "x64", "runner": "macos-15-intel", "triple": "x86_64-apple-darwin"},
]
out.append(("build-macos matrix holds exactly the macos-15 arm64 and macos-15-intel x64 entries", mac is not None and inc == want_inc))
out.append(("build-macos runs on ${{ matrix.runner }}", mac is not None and mac.get("runs-on") == "${{ matrix.runner }}"))
mr = runs("build-macos") if mac else ""
for frag in ["codesign --verify", "lipo -archs", "cstan-$VERSION-macos-$ARCH --version", "cstan-dash-$VERSION-macos-$ARCH --version"]:
    out.append(("build-macos runs '" + frag + "'", frag in mr))
out.append(("build-macos sets MACOSX_DEPLOYMENT_TARGET", mac is not None and bool((mac.get("env") or {}).get("MACOSX_DEPLOYMENT_TARGET"))))
if len(pub) == 1:
    out.append(("the publish job needs build-macos", "build-macos" in need(pub[0])))
    out.append(("the publish job's if is exactly github.event_name == 'push'", str(jobs[pub[0]].get("if", "")).strip() == "github.event_name == 'push'"))
else:
    out.append(("the publish job needs build-macos", False))
    out.append(("the publish job's if is exactly github.event_name == 'push'", False))
on = doc.get("on", doc.get(True))
out.append(("workflow_dispatch is a trigger", isinstance(on, dict) and "workflow_dispatch" in on))
bad = []
for j in jobs:
    if j in pub:
        continue
    for st in steps(j):
        if "GITHUB_REF_NAME" in st.get("run", "") and not (j == "build" and st.get("id") == "version"):
            bad.append(j)
out.append(("no job other than the version step and publish reads GITHUB_REF_NAME", not bad))
ver = [st.get("run", "") for st in steps("build") if st.get("id") == "version"] if "build" in jobs else []
out.append(("the version step still exits on 'does not match VERSION'", bool(ver) and "does not match VERSION" in ver[0] and "exit 1" in ver[0]))
for jn in (x64 + arm):
    dl = [st for st in steps(jn) if str(st.get("uses", "")).startswith("actions/download-artifact")]
    out.append((jn + " downloads with name: binaries", bool(dl) and all((st.get("with") or {}).get("name") == "binaries" and "pattern" not in (st.get("with") or {}) for st in dl)))
pd = [st for st in steps(pub[0]) if str(st.get("uses", "")).startswith("actions/download-artifact")] if len(pub) == 1 else []
out.append(("publish downloads pattern: binaries* with merge-multiple", bool(pd) and all((st.get("with") or {}).get("pattern") == "binaries*" and (st.get("with") or {}).get("merge-multiple") is True for st in pd)))
for name, ok in out:
    print(("ok   " if ok else "FAIL ") + name)
PY
)" || STRUCT="FAIL the workflow structure could not be read"
  printf '%s\n' "$STRUCT"
  N="$(printf '%s\n' "$STRUCT" | grep -c "^FAIL " || true)"
  FAILURES=$((FAILURES + N))
fi

# --- text rules ----------------------------------------------------------------
for forbidden in setup-node 'npm ' 'npm$' 'node ' 'node-version' '\.tgz' 'cstan-front-' 'cstan-node' 'cstan-daemon'; do
  if has "$forbidden"; then fail "release.yml mentions '$forbidden'"; else pass "release.yml has no '$forbidden'"; fi
done
has 'cargo zigbuild' && pass "builds with cargo zigbuild" || fail "builds with cargo zigbuild"
has 'x86_64-unknown-linux-musl' && has 'aarch64-unknown-linux-musl' && pass "both musl triples" || fail "both musl triples"
for asset in 'cstan-\$VERSION-linux-x64' 'cstan-\$VERSION-linux-arm64' 'cstan-dash-\$VERSION-linux-x64' 'cstan-dash-\$VERSION-linux-arm64'; do
  has "$asset" && pass "publishes $asset" || fail "publishes $asset"
done
for asset in 'cstan-\$VERSION-macos-x64' 'cstan-\$VERSION-macos-arm64' 'cstan-dash-\$VERSION-macos-x64' 'cstan-dash-\$VERSION-macos-arm64'; do
  has "$asset" && pass "publishes $asset" || fail "publishes $asset"
done
has 'sha256sum' && has 'SHA256SUMS' && pass "writes SHA256SUMS" || fail "writes SHA256SUMS"
has 'check-version.sh' && pass "checks VERSION against CHANGELOG.md" || fail "checks VERSION against CHANGELOG.md"

# --- local builds ----------------------------------------------------------------
if [ "${1:-}" = "--build" ]; then
  command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1 || {
    echo "--build needs cargo-zigbuild and zig" >&2
    exit 1
  }
  VERSION="$(tr -d ' \t\r\n' <"$ROOT/VERSION")"
  OUT="${CARGO_TARGET_DIR:-}"
  [ -n "$OUT" ] || { echo "--build needs CARGO_TARGET_DIR outside the checkout" >&2; exit 1; }
  for pair in x64:x86_64-unknown-linux-musl:x86-64 arm64:aarch64-unknown-linux-musl:aarch64; do
    arch="${pair%%:*}"
    rest="${pair#*:}"
    triple="${rest%%:*}"
    want="${rest#*:}"
    for crate in cstan dash; do
      if [ "$crate" = cstan ]; then
        dir=rust
        bin=cstan
        build="cargo zigbuild --release --locked -p cstan-front --target $triple"
      else
        dir=dash
        bin=cstan-dash
        build="cargo zigbuild --release --locked --target $triple"
      fi
      # shellcheck disable=SC2086
      if (cd "$ROOT/$dir" && CSTAN_VERSION="$VERSION" CARGO_TARGET_DIR="$OUT/$crate" nice -n 19 $build -j 2 >"$OUT/$bin-$arch.log" 2>&1); then
        pass "built $bin for $triple"
      else
        fail "built $bin for $triple (see $OUT/$bin-$arch.log)"
        continue
      fi
      file_out="$(file -b "$OUT/$crate/$triple/release/$bin")"
      case "$file_out" in
        *"$want"*"statically linked"* | *"$want"*"static-pie linked"*) pass "$bin $arch: $file_out" ;;
        *) fail "$bin $arch is not a static $want executable: $file_out" ;;
      esac
      if [ "$arch" = x64 ]; then
        runner=""
      elif command -v qemu-aarch64 >/dev/null 2>&1; then
        runner="qemu-aarch64"
      elif command -v qemu-aarch64-static >/dev/null 2>&1; then
        runner="qemu-aarch64-static"
      else
        echo "skip $bin arm64 run: qemu-aarch64 is not installed (the arm64 runner checks it in CI)"
        continue
      fi
      got="$($runner "$OUT/$crate/$triple/release/$bin" --version 2>&1)" || true
      case "$got" in
        *"$VERSION") pass "$bin $arch --version is '$got'" ;;
        *) fail "$bin $arch --version printed '$got'" ;;
      esac
    done
  done
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES release workflow check(s) failed"
  exit 1
fi
echo "release workflow checks passed"
