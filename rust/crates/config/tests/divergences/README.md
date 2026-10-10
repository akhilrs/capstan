# Divergences of the configuration replay

Overlays of `tests/config_parity.rs` (`capstan-parity-overlay`, schema in `rust/crates/parity-overlay/README.md`). The
fixtures are `config-edge-cases.json` and `config-corpus.json` (in `tests/parity`); `expected` has the shape of an entry of `config-expected.json`
(`{kind: "ok", sha256, bytes}` over the RoleConfig JSON with `$ROOT`, or `{kind: "error", message}`).

Three are the `[daemon]` table, retired at cutover: Rust is the only daemon (fixture `config-edge-cases.json`).

- `edge-daemon-node`: `implementation = "node"` is a configuration error that says the Node daemon was removed.
- `edge-daemon-rust`: loads, with a warning, and the RoleConfig JSON has no `daemon` member.
- `edge-daemon-empty`: loads, and the JSON has no `daemon` member.

The other `[daemon]` cases (bad value, wrong type, unknown key, not a table) give Node's text unchanged.

Two are the researcher role's permission mode (fixture `config-corpus.json`): Node refused every mode but `default`; Rust
accepts any, so both configs load.

- `captured-0513` (`researcher-mode-acceptEdits`): the researcher with `permission_mode = "acceptEdits"` loads.
- `captured-0514` (`researcher-mode-auto`): the researcher with `permission_mode = "auto"` loads.
