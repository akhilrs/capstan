# Divergences of the launcher replay

The overlays of `tests/parity.rs` (`capstan-parity-overlay`) go here as one `*.json` file each. There are none.

## The PM prompt is Rust-owned text (plan-33, PM-194)

The frozen sequences record each prompt that the launcher types into the pane (or passes to the host) by its byte length and
SHA-256. The PM prompt (`crates/config/src/prompts/pm_reference.txt`) is no longer Node's text, and any edit of it would change
every one of those numbers. No number is hard-coded and no sequence is overlaid: `normal` in `tests/common/mod.rs` recognises a
PM prompt (a long string that starts with the opening of the prompt the config crate builds, either as it is or inside the
`developer_instructions="..."` TOML string that Codex gets) and records it as the marker `{"$pmPrompt": true}`; `first_difference`
accepts the marker where the frozen value is a digest. The opening is read from the current prompt, so editing the PM prompt
needs no test edit. Every other prompt and every other byte count or call still compares by digest.

What is not compared any more: the bytes after the opening of a string that carries a PM prompt, including the restart summary
that follows it in a replacement's prompt (the summary text is covered by the kernel's restart tests). The prompt text itself is
covered by `crates/config/tests/prompts_parity.rs` (see `crates/config/tests/divergences/README.md`).
