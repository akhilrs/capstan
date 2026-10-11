# Divergences of the `cstan` replays

The overlays of `tests/transcripts.rs` and `tests/local.rs` (`capstan-parity-overlay`, schema in
`rust/crates/parity-overlay/README.md`) go here as one `*.json` file each. The native `cstan` prints, for
every recorded transcript, what Node printed, except for the usage text (below).

The one place Rust differs from Node has no Node fixture to overlay (Node ran its own dashboard when it found no
`cstan-dash`), so `tests/dash_handoff.rs` holds it; `docs/parity-fixtures.md` lists it.

The starter `capstan.toml` comment on the Researcher changed (the role may run in any permission mode). Ten `init`
transcripts record the starter in their expected `files`; an overlay replaces only stdout, stderr and exit, so
`tests/local.rs` rewrites the old comment block to the new one in the expected file text
(`STARTER_RESEARCHER_COMMENT_NODE` to `STARTER_RESEARCHER_COMMENT_RUST`) before comparing. The fixtures are not edited.
The `layout` contents of the transcripts hold the old starter as input only; it is never compared.

## The usage text

`cstan help` is grouped and a usage error prints a short hint (the syntax of the command, and a pointer to `cstan help`)
instead of Node's one-line usage, printed twice. 100 recorded cases hold Node's line, so they are not overlaid one by one:
`rewrite_node_usage` in `tests/support/mod.rs` turns Node's usage line on standard output into `usage::help_text()` and
Node's doubled line on standard error into `usage::hint(args)` before comparing. `tests/usage_table.rs` checks the text
itself (every command of Node's line, kept in `tests/support/node-usage.txt`, is in the table; 80 columns; the man page).
`help-extra-argument.json` is overlaid because `cstan help x` is now an unknown-command error (exit 2), where Node ignored
the word and printed the usage with exit 0.
