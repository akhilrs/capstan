# Divergences of the `cstan` replays

The overlays of `tests/transcripts.rs` and `tests/local.rs` (`capstan-parity-overlay`, schema in
`rust/crates/parity-overlay/README.md`) go here as one `*.json` file each. The native `cstan` prints, for
every recorded transcript, what Node printed, except for the usage text (below).

The one place Rust differs from Node has no Node fixture to overlay (Node ran its own dashboard when it found no
`cstan-dash`), so `tests/dash_handoff.rs` holds it; `docs/parity-fixtures.md` lists it.

The starter `capstan.toml` was rewritten as a short file in a fixed section order (PM-284). The ten `init` transcripts that
record the starter in their expected `files` were regenerated from the Rust binary (`CSTAN_REGEN_LOCAL=init-,usage-init-`);
only their `files` changed, and `tests/local.rs` compares them as recorded. The `layout` contents of the transcripts hold
the old starter as input only; it is never compared.

## The usage text

`cstan help` is grouped and a usage error prints a short hint (the syntax of the command, and a pointer to `cstan help`)
instead of Node's one-line usage, printed twice. 100 recorded cases hold Node's line, so they are not overlaid one by one:
`rewrite_node_usage` in `tests/support/mod.rs` turns Node's usage line on standard output into `usage::help_text()` and
Node's doubled line on standard error into `usage::hint(args)` before comparing. `tests/usage_table.rs` checks the text
itself (every command of Node's line, kept in `tests/support/node-usage.txt`, is in the table; 80 columns; the man page).
`help-extra-argument.json` is overlaid because `cstan help x` is now an unknown-command error (exit 2), where Node ignored
the word and printed the usage with exit 0.

## `cstan cleanup`

`cleanup` is a Rust-only command (the daemon route is in `RUST_ONLY_ROUTES`), so no Node transcript holds it. It is in the
command table, so `cstan help`, `cstan help cleanup` and `cstan help --man` list it. `tests/support/node-usage.txt` stays
Node's recorded line (the replay turns that line into the current help), so it does not list `cleanup`;
`tests/usage_table.rs` checks the new entries and the man page.
