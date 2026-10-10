# Divergences of the `cstan` replays

The overlays of `tests/transcripts.rs` and `tests/local.rs` (`capstan-parity-overlay`, schema in
`rust/crates/parity-overlay/README.md`) go here as one `*.json` file each. There are none: the native `cstan` prints, for
every recorded transcript, what Node printed.

The one place Rust differs from Node has no Node fixture to overlay (Node ran its own dashboard when it found no
`cstan-dash`), so `tests/dash_handoff.rs` holds it; `docs/parity-fixtures.md` lists it.
