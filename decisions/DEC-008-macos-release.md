# DEC-008: macOS release binaries (native runners, ad-hoc signing)

**Tracker:** PM-536 (plan-186)
**Status:** accepted with plan-186.
**Decision:** Release macOS arm64 and x64 binaries of `cstan` and `cstan-dash`, built natively by the Release workflow on GitHub's `macos-15` (arm64) and `macos-15-intel` (x64) runners with `MACOSX_DEPLOYMENT_TARGET=11.0`, ad-hoc signed, not notarized. `install.sh` installs them in the layout Linux already uses.

## Context

Until now Capstan published four static musl Linux binaries. The runtime gaps on macOS were closed by [DEC-007](DEC-007-macos-process-information.md) and its follow-ups, so the code runs on macOS; what was missing was a release, an installer path and a CI check.

## Options

1. **Cross-build macOS from Linux** (cargo-zigbuild or osxcross). No macOS runner needed, but it needs an SDK that is not freely redistributable, and a binary that was never run on macOS proves little.
2. **Build natively on macOS runners (chosen).** Each binary is built, signed, checked with `lipo -archs` and run with `--version` on the machine type it targets. The deployment target `11.0` keeps the binaries running on macOS 11 and later while the runners are macOS 15.
3. **One universal binary.** Larger assets and a `lipo` step, and the installer would still need to know the OS; two single-arch assets match the Linux naming.

## Decisions

- **Assets:** `cstan-<version>-macos-<arm64|x64>` and `cstan-dash-<version>-macos-<arm64|x64>`, listed in `SHA256SUMS` beside the four Linux assets.
- **Signing:** ad-hoc (`codesign --force --sign -`), because Apple silicon refuses unsigned binaries. No Developer ID.
- **Quarantine and notarization:** a file fetched by `curl` has no quarantine flag, and `install.sh` also clears `com.apple.quarantine` on what it installs, so `curl | sh` works with no prompt. A binary downloaded in a browser is quarantined and needs `xattr -d com.apple.quarantine <file>`. Notarization is not done.
- **Layout:** XDG paths are kept on macOS (`~/.local/share/capstan`, `~/.local/bin`) so docs, scripts and `cstan` itself have one layout.
- **Rosetta:** a shell under Rosetta reports `x86_64`; `install.sh` reads `sysctl.proc_translated` and installs the arm64 binary.
- **Checks:** the Release workflow also runs on `workflow_dispatch` as a dry run; CI's `macos` job on `macos-15` runs `scripts/check.sh unit`, an `x86_64-apple-darwin` cargo check and `scripts/test-install.sh`; a Linux developer runs `scripts/check-darwin.sh`. `scripts/smoke-binary.sh`, `scripts/daemon-survivors.sh` and the integration tests stay Linux-only.

## Consequences

- If GitHub retires `macos-15-intel`, the x64 asset is built on `macos-15` for `x86_64-apple-darwin` and run under Rosetta.
- The known macOS limit PM-359 (DEC-007) is unchanged.

## Follow-ups

- Developer ID signing and notarization, only if Capstan is distributed through browser download or Homebrew.
- macOS integration suites (the Linux-only ones), and the first release with macOS assets checked by hand against these docs.
