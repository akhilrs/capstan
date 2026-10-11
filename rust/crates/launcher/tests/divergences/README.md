# Divergences of the launcher replay

The overlays of `tests/parity.rs` (`capstan-parity-overlay`) go here as one `*.json` file each. There are none.

## The PM prompt has a `cstan cleanup` line (plan-33)

The frozen sequences record the PM prompt that the launcher types into the pane (or passes to the host) by its byte length
and SHA-256. Rust's PM prompt has one more line than Node's, the `cstan cleanup` bullet of `pm_reference.txt`. No number is
hard-coded and no sequence is overlaid: `normal` in `tests/common/mod.rs` drops that one line (the one starting with
``- `cstan cleanup --dry-run` ``, also where Codex carries the prompt in a TOML string with `\u000a` newlines) from a string
before it measures and hashes it, so the Rust prompt is compared with Node's text. Editing the line still matches; any other
change to the PM prompt, and any other byte count or call, still fails. The new text itself is covered by
`crates/config/tests/prompts_parity.rs` (see `crates/config/tests/divergences/README.md`).
