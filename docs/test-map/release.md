# Test map: release, installer and packaging (plan-30 `release-rust`)

Where the Node release and packaging tests went when releases became two musl Rust binaries. Each file is **ported**
(to Rust or to a shell script, with the names), **retired** (the thing it tested no longer exists), or **frozen** (a Node
SEA test that stays green until plan D deletes it, and is not extended).

## Summary

| Node file | Disposition |
| --- | --- |
| release-notes.test.ts (deleted) | **ported** to `tools/release/tests/notes.rs` (below) |
| `test/packaging.test.ts` | release cases **retired**; the rest **frozen**; one case added (`VERSION` matches `package.json`) |
| `test/sea.test.ts` | **frozen**: tests the Node SEA helpers in `src/sea.ts`; stays green until plan D deletes it |
| `test/operator-restart-sea.test.ts` | **frozen**: restart of the Node standalone binary; skipped without a binary, stays green until plan D deletes it |

## `release-notes.test.ts` → `tools/release/tests/notes.rs`

`tools/release` is a separate cargo workspace (own `Cargo.lock`, no dependencies) that ports `scripts/release-notes.mjs`.
Run it with `cd tools/release && cargo test --locked`. `scripts/release.sh` calls its binary, `cstan-release`.

| Node case | Rust test |
| --- | --- |
| level: fix and perf are patch, feat is minor, breaking is major | `level_fix_and_perf_are_patch_feat_is_minor_breaking_is_major` |
| level: breaking on 0.y.z is minor | `level_breaking_on_0_y_z_is_minor` |
| level: only docs/chore commits, merges and junk give null | `level_only_docs_chore_commits_merges_and_junk_give_none` |
| parseCommit reads scope, breaking footer and rejects junk | `parse_commit_reads_scope_breaking_footer_and_rejects_junk` |
| release-notes accepts and rejects the same subjects as `src/conventions` | `accepts_and_rejects_the_subjects_src_conventions_does` (the same subjects as a fixed table: the Rust test cannot import the TypeScript rule) |
| changelog section groups by type with scope and short sha | `changelog_section_groups_by_type_with_scope_and_short_sha` |
| lastReleaseTag picks the highest vX.Y.Z reachable from HEAD | `last_release_tag_picks_the_highest_vxyz_tag_reachable_from_head` |
| changelog drops an exact duplicate and keeps the first | `changelog_drops_an_exact_duplicate_and_keeps_the_first` |
| changelog treats case, spacing and trailing punctuation as the same subject | `changelog_treats_case_spacing_and_trailing_punctuation_as_the_same_subject` |
| changelog keeps different subjects that share a short prefix | `changelog_keeps_different_subjects_that_share_a_short_prefix` |
| changelog merges a scoped, cut-off and re-typed repeat into one entry | `changelog_merges_a_scoped_cut_off_and_retyped_repeat_into_one_entry` |
| changelog keeps a scope and breaking flag from either repeat | `changelog_keeps_a_scope_and_breaking_flag_from_either_repeat` |

## `packaging.test.ts`

Retired with the npm and SEA release flow (they tested `scripts/release.mjs` in a sandbox with a stub `npm`):
`scripts.release`, `releaseSandbox` and every case built on it (shrinkwrap removal, `--allow-dirty-lock`, restore on a
failed build, the nested release dir and `SHA256SUMS`, `--no-dash`, `--no-front`, `--dry-run`, the bump, `--version`,
the refusals). The cases that stay (`--version`/`--help` output, usage failures, the `package.json` invariants,
`build:cli`) are frozen with the Node CLI. New: `VERSION is the version package.json and the lockfile carry`.

The release flow is covered instead by:

| Area | Where |
| --- | --- |
| `release.sh` options, the bump of `VERSION`/`package.json`/lock, the CHANGELOG section, the commit and tag, the dirty-tree refusal | `scripts/release.sh`, run in a scratch clone (see the report of the package); the docs table is checked against its options by `test/prompts.test.ts` |
| version agreement | `scripts/check-version.sh` (run by `scripts/smoke-binary.sh`) and the packaging case above |
| the workflow | `scripts/check-release-workflow.sh` (actionlint or a parser, then the rules); `--build` builds both triples with `cargo-zigbuild` |
| the installer | `scripts/test-install.sh` |
| the binaries | `scripts/smoke-binary.sh` |

## What runs where

| Check | Runs on |
| --- | --- |
| `tools/release` tests, `check-version.sh`, `check-release-workflow.sh`, `test-install.sh` | any Linux machine with `sh`, `git`, `curl`, `python3` (workflow parse) |
| `check-release-workflow.sh --build`: both triples built with `cargo-zigbuild` and checked with `file` | a machine with `cargo-zigbuild` and `zig` |
| the arm64 binaries **run** | under `qemu-aarch64` when installed, otherwise only the `verify-arm64` job of the Release workflow on the `ubuntu-24.04-arm` runner |
| the real GitHub release, the real arm64 runner | CI on a pushed tag (not run locally) |
