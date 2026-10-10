# Divergences of the `cstan` replays

The overlays of `tests/transcripts.rs` and `tests/local.rs` (`capstan-parity-overlay`, schema in
`rust/crates/parity-overlay/README.md`) go here as one `*.json` file each. There are none: the native `cstan` prints, for
every recorded transcript, what Node printed.

The one place Rust differs from Node has no Node fixture to overlay (Node ran its own dashboard when it found no
`cstan-dash`), so `tests/dash_handoff.rs` holds it; `docs/parity-fixtures.md` lists it.

The starter `capstan.toml` comment on the Researcher changed (the role may run in any permission mode). Ten `init`
transcripts record the starter in their expected `files`; an overlay replaces only stdout, stderr and exit, so
`tests/local.rs` rewrites the old comment block to the new one in the expected file text
(`STARTER_RESEARCHER_COMMENT_NODE` to `STARTER_RESEARCHER_COMMENT_RUST`) before comparing. The fixtures are not edited.
The `layout` contents of the transcripts hold the old starter as input only; it is never compared.
